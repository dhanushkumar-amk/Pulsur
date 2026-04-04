use std::collections::HashMap;
use http_server::{Request as GatewayRequest, Response as GatewayResponse, Method as HttpMethod};
use futures::future::BoxFuture;
use std::sync::Arc;
use uuid::Uuid;
use serde::{Deserialize, Serialize};

pub mod auth;
pub use auth::*;


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
    /// Internal headers to strip from the response before sending to client.
    #[serde(default)]
    pub strip_headers: Vec<String>,
    /// Prefix to strip from the incoming request path (e.g. "/api/v1").
    #[serde(default)]
    pub prefix_strip: Option<String>,
    /// Headers to inject into every response (e.g. CORS).
    #[serde(default)]
    pub inject_response_headers: HashMap<String, String>,
    /// JSON request body key mappings (old_key -> new_key).
    #[serde(default)]
    pub body_transformations: HashMap<String, String>,
    /// JSON response body fields to remove (e.g. ["internal_ptr", "debug_info"]).
    #[serde(default)]
    pub response_body_strip: Vec<String>,
}

/// A specialized plugin for Phase 14: Request and Response Header/Body transformations.
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

            // A. Inject X-Request-ID (UUID v4)
            if !ctx.request.headers.contains_key("x-request-id") {
                let rid = Uuid::new_v4().to_string();
                ctx.request.headers.insert("x-request-id".to_string(), rid.clone());
                ctx.metadata.insert("request_id".to_string(), rid);
            }

            // B. Inject X-Forwarded-For (Appends client IP)
            // Note: In a production server, 127.0.0.1 would be replaced by the actual peer IP.
            let xff = match ctx.request.headers.get("x-forwarded-for") {
                Some(current) => format!("{}, 127.0.0.1", current),
                None => "127.0.0.1".to_string(),
            };
            ctx.request.headers.insert("x-forwarded-for".to_string(), xff);

            // C. Path Rewriting (Prefix strip)
            if let Some(prefix) = &self.config.prefix_strip {
                if ctx.request.path.starts_with(prefix) {
                    let mut new_path = ctx.request.path[prefix.len()..].to_string();
                    if new_path.is_empty() || !new_path.starts_with('/') {
                        new_path.insert(0, '/');
                    }
                    tracing::info!("Rewriting path: {} -> {}", ctx.request.path, new_path);
                    ctx.request.path = new_path;
                }
            }

            // D. Request Body Transformation (JSON key remapping)
            if !self.config.body_transformations.is_empty() && !ctx.request.body.is_empty() {
                if let Ok(mut json) = serde_json::from_slice::<serde_json::Value>(&ctx.request.body) {
                    if let Some(obj) = json.as_object_mut() {
                        let mut changed = false;
                        for (old_key, new_key) in &self.config.body_transformations {
                            if let Some(val) = obj.remove(old_key) {
                                tracing::info!("Transforming req field: {} -> {}", old_key, new_key);
                                obj.insert(new_key.clone(), val);
                                changed = true;
                            }
                        }
                        if changed {
                            ctx.request.body = serde_json::to_vec(&json).unwrap_or_else(|_| ctx.request.body.clone());
                        }
                    }
                }
            }

            // ──────────────────────────────────────────────────
            // 2. Execution
            // ──────────────────────────────────────────────────
            let mut res = next.run(ctx).await;

            // ──────────────────────────────────────────────────
            // 3. Response Modification (Phase 2)
            // ──────────────────────────────────────────────────

            // E. Inject Response Headers (CORS, etc.)
            for (k, v) in &self.config.inject_response_headers {
                res.headers.insert(k.to_lowercase(), v.clone());
            }

            // F. Strip internal headers
            for h in &self.config.strip_headers {
                res.headers.remove(&h.to_lowercase());
            }

            // G. Response Body Transformation (Sensitve field strip)
            if !self.config.response_body_strip.is_empty() && !res.body.is_empty() {
                if let Ok(mut json) = serde_json::from_slice::<serde_json::Value>(&res.body) {
                    if let Some(obj) = json.as_object_mut() {
                        let mut changed = false;
                        for field in &self.config.response_body_strip {
                            if obj.remove(field).is_some() {
                                tracing::info!("Stripping response field: {}", field);
                                changed = true;
                            }
                        }
                        if changed {
                            res.body = serde_json::to_vec(&json).unwrap_or_else(|_| res.body.clone());
                        }
                    }
                }
            }

            res
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

            tracing::info!("Forwarding to: {} {}", method, target_url);
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

#[cfg(test)]
mod tests {
    use super::*;
    use http_server::{Request as GatewayRequest, Method, HttpVersion, Response as GatewayResponse};

    fn mock_context(path: &str, body: Option<&str>) -> Context {
        let mut headers = HashMap::new();
        if body.is_some() {
            headers.insert("content-type".to_string(), "application/json".to_string());
        }
        let req = GatewayRequest {
            method: Method::GET,
            path: path.to_string(),
            version: HttpVersion::Http11,
            headers,
            params: HashMap::new(),
            body: body.map(|s| s.as_bytes().to_vec()).unwrap_or_default(),
        };
        Context::new(req)
    }

    struct MockNext;
    impl Plugin for MockNext {
        fn call<'a>(&'a self, _ctx: &'a mut Context, _next: Next) -> BoxFuture<'a, GatewayResponse> {
            Box::pin(async { 
                let mut res = GatewayResponse::new(200);
                res.headers.insert("content-type".to_string(), "application/json".to_string());
                res.headers.insert("x-internal-id".to_string(), "secret".to_string());
                res.body = r#"{"status":"ok","debug_info":"some_leak"}"#.as_bytes().to_vec();
                res 
            })
        }
    }

    #[tokio::test]
    async fn test_transform_plugin_full() {
        let yaml = r#"
prefix_strip: "/api/v1"
inject_response_headers:
  Access-Control-Allow-Origin: "*"
strip_headers:
  - "x-internal-id"
body_transformations:
  "user_id": "id"
response_body_strip:
  - "debug_info"
"#;
        let plugin = TransformPlugin::from_config(yaml).unwrap();
        let pipeline = Pipeline::new(vec![Box::new(plugin), Box::new(MockNext)]);

        let ctx = mock_context("/api/v1/users", Some(r#"{"user_id":123}"#));
        let res = pipeline.execute(ctx).await;

        // Verify Response (injected headers and stripped body)
        assert_eq!(res.headers.get("access-control-allow-origin").unwrap(), "*");
        assert!(res.headers.get("x-internal-id").is_none());

        let body_val: serde_json::Value = serde_json::from_slice(&res.body).unwrap();
        assert!(body_val.get("debug_info").is_none());
        assert_eq!(body_val.get("status").unwrap(), "ok");
    }
}

