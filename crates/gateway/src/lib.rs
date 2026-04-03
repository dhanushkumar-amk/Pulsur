use std::collections::HashMap;
use http_server::{Request as GatewayRequest, Response as GatewayResponse, Method as HttpMethod};
use futures::future::BoxFuture;
use std::sync::Arc;
use uuid::Uuid;
use serde::{Deserialize, Serialize};

/// Context for the gateway request pipeline.
pub struct Context {
    pub request: GatewayRequest,
    pub response: Option<GatewayResponse>,
    pub metadata: HashMap<String, String>,
    pub upstream_url: Option<String>,
}

impl Context {
    pub fn new(request: GatewayRequest) -> Self {
        Self {
            request,
            response: None,
            metadata: HashMap::new(),
            upstream_url: None,
        }
    }
}

/// The plugin chain's "next" handle.
pub struct Next {
    pub(crate) plugins: Arc<Vec<Box<dyn Plugin>>>,
    pub(crate) index: usize,
}

impl Next {
    pub fn run<'a>(self, ctx: &'a mut Context) -> BoxFuture<'a, GatewayResponse> {
        Box::pin(async move {
            if self.index < self.plugins.len() {
                let plugin = &self.plugins[self.index];
                let next = Next {
                    plugins: Arc::clone(&self.plugins),
                    index: self.index + 1,
                };
                plugin.call(ctx, next).await
            } else {
                ctx.response.take().unwrap_or_else(|| GatewayResponse::new(404))
            }
        })
    }
}

/// Trait for gateway plugins.
pub trait Plugin: Send + Sync {
    fn call<'a>(&'a self, ctx: &'a mut Context, next: Next) -> BoxFuture<'a, GatewayResponse>;
}

/// Configuration for the TransformPlugin.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct TransformConfig {
    #[serde(default)]
    pub strip_headers: Vec<String>,
    #[serde(default)]
    pub prefix_strip: Option<String>,
    #[serde(default)]
    pub inject_response_headers: HashMap<String, String>,
    #[serde(default)]
    pub body_transformations: HashMap<String, String>, // old_key -> new_key
}

/// A specialized plugin for Phase 14: Header and Body transformations.
pub struct TransformPlugin {
    config: TransformConfig,
}

impl TransformPlugin {
    pub fn new(config: TransformConfig) -> Self {
        Self { config }
    }

    pub fn from_config(yaml: &str) -> anyhow::Result<Self> {
        let config: TransformConfig = serde_yaml::from_str(yaml)?;
        Ok(Self::new(config))
    }
}

impl Plugin for TransformPlugin {
    fn call<'a>(&'a self, ctx: &'a mut Context, next: Next) -> BoxFuture<'a, GatewayResponse> {
        Box::pin(async move {
            // ──────────────────────────────────────────────────
            // 1. Request Modification (Phase 1)
            // ──────────────────────────────────────────────────

            // A. Inject X-Request-ID
            let request_id = Uuid::new_v4().to_string();
            ctx.request.headers.insert("x-request-id".to_string(), request_id.clone());
            ctx.metadata.insert("request_id".to_string(), request_id);

            // B. Inject X-Forwarded-For (Mock logic for local)
            if !ctx.request.headers.contains_key("x-forwarded-for") {
                ctx.request.headers.insert("x-forwarded-for".to_string(), "127.0.0.1".to_string());
            }

            // C. Path Rewriting (Prefix strip)
            if let Some(prefix) = &self.config.prefix_strip {
                if ctx.request.path.starts_with(prefix) {
                    let new_path = &ctx.request.path[prefix.len()..];
                    let final_path = if new_path.is_empty() { "/" } else { new_path };
                    tracing::info!("Rewriting path: {} -> {}", ctx.request.path, final_path);
                    ctx.request.path = final_path.to_string();
                }
            }

            // D. Request Body Transformation (JSON)
            if !self.config.body_transformations.is_empty() && !ctx.request.body.is_empty() {
                match serde_json::from_slice::<serde_json::Value>(&ctx.request.body) {
                    Ok(mut json) => {
                        if let Some(obj) = json.as_object_mut() {
                            for (old_key, new_key) in &self.config.body_transformations {
                                if let Some(val) = obj.remove(old_key) {
                                    tracing::info!("Renaming field: {} -> {}", old_key, new_key);
                                    obj.insert(new_key.clone(), val);
                                }
                            }
                            ctx.request.body = serde_json::to_vec(&json).unwrap_or_default();
                        }
                    }
                    Err(e) => tracing::warn!("Failed to parse request JSON for transformation: {}", e),
                }
            }

            // ──────────────────────────────────────────────────
            // 2. Call Next
            // ──────────────────────────────────────────────────
            let mut response = next.run(ctx).await;

            // ──────────────────────────────────────────────────
            // 3. Response Modification (Phase 2)
            // ──────────────────────────────────────────────────

            // E. Inject configured response headers (CORS, etc.)
            for (k, v) in &self.config.inject_response_headers {
                response.headers.insert(k.to_lowercase(), v.clone());
            }

            // F. Response Header Stripping
            for header in &self.config.strip_headers {
                response.headers.remove(&header.to_lowercase());
            }

            // G. Response Body Transformation (Simple field filter example)
            if !response.body.is_empty() {
                if let Ok(mut json) = serde_json::from_slice::<serde_json::Value>(&response.body) {
                    if let Some(obj) = json.as_object_mut() {
                        // Example: remove keys ending in '_internal' or specifically 'debug_info'
                        obj.remove("debug_info");
                        obj.remove("internal_ptr");
                        response.body = serde_json::to_vec(&json).unwrap_or_default();
                    }
                }
            }

            response
        })
    }
}

/// Generic Passthrough Plugin to send the context to an upstream.
pub struct PassthroughPlugin {
    client: reqwest::Client,
}

impl Default for PassthroughPlugin {
    fn default() -> Self {
        Self { client: reqwest::Client::new() }
    }
}

impl Plugin for PassthroughPlugin {
    fn call<'a>(&'a self, ctx: &'a mut Context, _next: Next) -> BoxFuture<'a, GatewayResponse> {
        Box::pin(async move {
            let upstream_url = ctx.upstream_url.as_ref().expect("Upstream URL missing");
            let target_url = format!("{}{}", upstream_url, ctx.request.path);

            let method = match ctx.request.method {
                HttpMethod::GET => reqwest::Method::GET,
                HttpMethod::POST => reqwest::Method::POST,
                _ => reqwest::Method::GET, // simplified
            };

            let mut req_builder = self.client.request(method, &target_url);
            for (k, v) in &ctx.request.headers {
                if k != "host" { req_builder = req_builder.header(k, v); }
            }
            req_builder = req_builder.body(ctx.request.body.clone());

            match req_builder.send().await {
                Ok(resp) => {
                    let mut res = GatewayResponse::new(resp.status().as_u16());
                    for (name, val) in resp.headers() {
                        if let Ok(v) = val.to_str() {
                            res.headers.insert(name.as_str().to_lowercase(), v.to_string());
                        }
                    }
                    res.body = resp.bytes().await.unwrap_or_default().to_vec();
                    res
                }
                Err(_) => GatewayResponse::new(502),
            }
        })
    }
}

/// Simple Pipeline to glue everything together.
pub struct Pipeline {
    plugins: Arc<Vec<Box<dyn Plugin>>>,
}

impl Pipeline {
    pub fn new(plugins: Vec<Box<dyn Plugin>>) -> Self {
        Self { plugins: Arc::new(plugins) }
    }

    pub async fn execute(&self, mut ctx: Context) -> GatewayResponse {
        let next = Next {
            plugins: Arc::clone(&self.plugins),
            index: 0,
        };
        next.run(&mut ctx).await
    }
}
