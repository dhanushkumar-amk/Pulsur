use std::collections::HashMap;
use http_server::{Request as GatewayRequest, Response as GatewayResponse, Method as HttpMethod, Router, HttpServer, ServerConfig};
use futures::FutureExt;
use futures::future::BoxFuture;
use std::sync::Arc;
use uuid::Uuid;
use serde::{Deserialize, Serialize};
use arc_swap::ArcSwap;
use std::path::Path;
use notify::{Watcher, RecursiveMode};
use dashmap::DashMap;
use std::time::Duration;

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

/// Root configuration for the Gateway.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct GatewayConfig {
    pub listen_addr: String,
    pub routes: Vec<RouteConfig>,
    pub auth: Option<AuthConfig>,
    pub transform: Option<TransformConfig>,
    pub rate_limit: Option<RateLimitConfig>,
    pub upstream: Option<UpstreamConfig>,
}

/// Configuration for an individual route.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct RouteConfig {
    pub method: String,
    pub path: String,
    pub upstream: String,
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

/// Configuration for the RateLimitPlugin.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct RateLimitConfig {
    /// Mapping of path prefix -> limit (req per 60s).
    #[serde(default)]
    pub limits: HashMap<String, u32>,
    /// Header to check for API keys (default: x-api-key)
    #[serde(default = "default_api_key_header")]
    pub api_key_header: String,
}

fn default_api_key_header() -> String { "x-api-key".to_string() }

/// A specialized plugin for Phase 16: Identity-based Sliding Window Rate Limiting.
pub struct RateLimitPlugin {
    config: RateLimitConfig,
    limiters: DashMap<u32, Arc<rate_limiter::SlidingWindowRateLimiter>>,
}

impl RateLimitPlugin {
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            config,
            limiters: DashMap::new(),
        }
    }

    fn limiter_for(
        &self,
        limit: u32,
    ) -> Result<Arc<rate_limiter::SlidingWindowRateLimiter>, rate_limiter::RateLimiterError> {
        if let Some(existing) = self.limiters.get(&limit) {
            return Ok(Arc::clone(existing.value()));
        }

        let limiter = Arc::new(rate_limiter::SlidingWindowRateLimiter::new(
            Duration::from_secs(60),
            limit as usize,
        )?);
        self.limiters.insert(limit, Arc::clone(&limiter));
        Ok(limiter)
    }
}

impl Plugin for RateLimitPlugin {
    fn call<'a>(&'a self, ctx: &'a mut Context, next: Next) -> BoxFuture<'a, GatewayResponse> {
        Box::pin(async move {
            // 1. Identify which limit applies (longest path match)
            let path = &ctx.request.path;
            let mut limit = None;
            for (prefix, l) in &self.config.limits {
                if path.starts_with(prefix) {
                    match limit {
                        Some((best_prefix_len, _)) if best_prefix_len >= prefix.len() => {}
                        _ => limit = Some((prefix.len(), *l)),
                    }
                }
            }

            // 2. If no limit is configured for this path, skip
            let limit = match limit {
                Some((_, configured_limit)) => configured_limit,
                None => return next.run(ctx).await,
            };

            // 3. Key Extraction (Order: Header -> Auth Claim -> IP)
            let key = if let Some(k) = ctx.request.headers.get(&self.config.api_key_header.to_lowercase()) {
                format!("api-key:{}", k)
            } else if let Some(sub) = ctx.metadata.get("auth_sub") {
                format!("user:{}", sub)
            } else {
                format!("ip:{}", ctx.request.peer_addr.ip())
            };

            // 4. Check with high-performance limiter
            let limiter = match self.limiter_for(limit) {
                Ok(limiter) => limiter,
                Err(e) => {
                    tracing::error!("invalid rate limiter configuration for path {}: {}", path, e);
                    return next.run(ctx).await;
                }
            };

            match limiter.check_key(&key) {
                Ok(status) => {
                    if !status.allowed {
                        tracing::warn!("Rate limit exceeded for key: {} (Path: {})", key, path);
                        metrics::counter!("gateway.rate_limit_exceeded_total", "key" => key.clone(), "path" => path.to_string()).increment(1);
                        
                        let mut res = GatewayResponse::new(429);
                        res.headers.insert("retry-after".to_string(), status.retry_after_secs.to_string());
                        res.headers.insert("x-ratelimit-limit".to_string(), status.limit.to_string());
                        res.headers.insert("x-ratelimit-remaining".to_string(), "0".to_string());
                        res.headers.insert("x-ratelimit-reset".to_string(), status.reset_after_secs.to_string());
                        
                        // User-friendly excess message
                        res.body = format!("Rate limit exceeded. Try again in {} seconds.", status.retry_after_secs)
                            .as_bytes().to_vec();
                        return res;
                    }

                    // 5. Success: Call next, but inject stats headers on the way back
                    let mut res = next.run(ctx).await;
                    res.headers.insert("x-ratelimit-limit".to_string(), status.limit.to_string());
                    res.headers.insert("x-ratelimit-remaining".to_string(), status.remaining.to_string());
                    res.headers.insert("x-ratelimit-reset".to_string(), status.reset_after_secs.to_string());
                    res
                }
                Err(e) => {
                    tracing::error!("Rate limiter hardware error: {}", e);
                    // Fail open or fail closed? Let's fail open to prevent downtime.
                    next.run(ctx).await
                }
            }
        })
    }
}
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
            let rid = ctx.request.headers.get("x-request-id")
                .cloned()
                .unwrap_or_else(|| {
                    let r = Uuid::new_v4().to_string();
                    ctx.request.headers.insert("x-request-id".to_string(), r.clone());
                    r
                });
            ctx.metadata.insert("request_id".to_string(), rid.clone());

            // B. Inject X-Forwarded-For (Appends client IP)
            let client_ip = ctx.request.peer_addr.ip().to_string();
            let xff = match ctx.request.headers.get("x-forwarded-for") {
                Some(current) => format!("{}, {}", current, client_ip),
                None => client_ip,
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
            res.headers.insert("x-request-id".to_string(), rid);
            for (k, v) in &self.config.inject_response_headers {
                res.headers.insert(k.to_lowercase(), v.clone());
            }

            // F. Strip internal headers
            for h in &self.config.strip_headers {
                res.headers.remove(&h.to_lowercase());
            }

            // G. Response Body Transformation (Sensitive field strip)
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


/// Configuration for the Upstream Forwarder (Phase 17).
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct UpstreamConfig {
    /// Request timeout in seconds (default: 30)
    #[serde(default = "default_timeout")]
    pub timeout_sec: u64,
    /// Maximum number of retries (default: 3)
    #[serde(default = "default_retries")]
    pub max_retries: u32,
    /// Base backoff in milliseconds (default: 100)
    #[serde(default = "default_base_backoff")]
    pub base_backoff_ms: u64,
}

fn default_timeout() -> u64 { 30 }
fn default_retries() -> u32 { 3 }
fn default_base_backoff() -> u64 { 100 }

impl Default for UpstreamConfig {
    fn default() -> Self {
        Self {
            timeout_sec: 30,
            max_retries: 3,
            base_backoff_ms: 100,
        }
    }
}

/// A highly resilient plugin for Phase 17: Forwarding with Tries, Timeouts, and Backoff.
pub struct ResilientPassthroughPlugin {
    config: UpstreamConfig,
    client: reqwest::Client,
}

impl ResilientPassthroughPlugin {
    pub fn new(config: UpstreamConfig) -> Self {
        Self {
            config,
            client: reqwest::Client::new(),
        }
    }
}

impl Plugin for ResilientPassthroughPlugin {
    fn call<'a>(&'a self, ctx: &'a mut Context, _next: Next) -> BoxFuture<'a, GatewayResponse> {
        Box::pin(async move {
            let upstream_url = ctx.upstream_url.as_ref().expect("Upstream URL missing");
            let target_url = format!("{}{}", upstream_url, ctx.request.path);
            let mut attempts = 0;

            loop {
                let method = match ctx.request.method {
                    HttpMethod::GET => reqwest::Method::GET,
                    HttpMethod::POST => reqwest::Method::POST,
                    HttpMethod::PUT => reqwest::Method::PUT,
                    HttpMethod::DELETE => reqwest::Method::DELETE,
                    _ => reqwest::Method::GET,
                };

                tracing::info!("Forwarding [Attempt {}/{}]: {} {}", 
                    attempts + 1, self.config.max_retries + 1, method, target_url);

                let mut req_builder = self.client.request(method, &target_url);
                for (k, v) in &ctx.request.headers {
                    if k != "host" { req_builder = req_builder.header(k, v); }
                }
                
                // Add retry count header for upstream visibility
                if attempts > 0 {
                    req_builder = req_builder.header("x-retry-count", attempts.to_string());
                }

                req_builder = req_builder.body(ctx.request.body.clone());

                // Wrap in timeout (Phase 17)
                let timeout_dur = std::time::Duration::from_secs(self.config.timeout_sec);
                let request_future = tokio::time::timeout(timeout_dur, req_builder.send());

                let result = request_future.await;

                match result {
                    // 1. Success (Network Level)
                    Ok(Ok(resp)) => {
                        let status = resp.status().as_u16();
                        
                        // If it's a 5xx error, we check for retry
                        if status >= 500 && attempts < self.config.max_retries {
                            attempts += 1;
                            let wait = self.calculate_backoff(attempts);
                            tracing::warn!("Upstream 5xx status: {}. Retrying in {}ms (Attempt {})...", 
                                status, wait, attempts);
                            tokio::time::sleep(std::time::Duration::from_millis(wait)).await;
                            continue;
                        }

                        // Parse the final response
                        let mut res = GatewayResponse::new(status);
                        for (name, val) in resp.headers() {
                            if let Ok(v) = val.to_str() {
                                res.headers.insert(name.as_str().to_lowercase(), v.to_string());
                            }
                        }
                        res.body = resp.bytes().await.unwrap_or_default().to_vec();
                        return res;
                    }

                    // 2. Timeout (Phase 17)
                    Ok(Err(e)) if e.is_timeout() => {
                        if attempts < self.config.max_retries {
                             attempts += 1;
                             let wait = self.calculate_backoff(attempts);
                             tracing::warn!("Upstream timeout. Retrying in {}ms (Attempt {})...", wait, attempts);
                             tokio::time::sleep(std::time::Duration::from_millis(wait)).await;
                             continue;
                        }
                        return self.bad_gateway("Upstream request timed out after all attempts.");
                    }
                    Err(_) => {
                        if attempts < self.config.max_retries {
                             attempts += 1;
                             let wait = self.calculate_backoff(attempts);
                             tracing::warn!("Upstream timeout. Retrying in {}ms (Attempt {})...", wait, attempts);
                             tokio::time::sleep(std::time::Duration::from_millis(wait)).await;
                             continue;
                        }
                        return self.bad_gateway("Upstream request timed out after all attempts.");
                    }

                    // 3. Network Failure or Unrecoverable Error
                    _ => {
                        if attempts < self.config.max_retries {
                             attempts += 1;
                             let wait = self.calculate_backoff(attempts);
                             tracing::warn!("Network error. Retrying in {}ms (Attempt {})...", wait, attempts);
                             tokio::time::sleep(std::time::Duration::from_millis(wait)).await;
                             continue;
                        }
                        return self.bad_gateway("Could not connect to upstream after all attempts.");
                    }
                }
            }
        })
    }
}

impl ResilientPassthroughPlugin {
    /// Zero-downtime exponential backoff: base * 2^attempt + jitter
    fn calculate_backoff(&self, attempt: u32) -> u64 {
        use rand::Rng;
        let base = self.config.base_backoff_ms;
        let multiplier = 2u64.pow(attempt - 1); // attempt 1 = 2^0 = 1, attempt 2 = 2^1 = 2
        let jitter = rand::thread_rng().gen_range(0..50);
        (base * multiplier) + jitter
    }

    fn bad_gateway(&self, msg: &str) -> GatewayResponse {
        let mut res = GatewayResponse::new(502);
        res.body = format!("502 Bad Gateway: {}", msg).as_bytes().to_vec();
        res
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

/// The main entry point for the Pulsar Gateway with hot-reload capability.
pub struct HotReloadGateway {
    pub config_path: String,
    pub pipeline: Arc<ArcSwap<Pipeline>>,
}

impl HotReloadGateway {
    /// Start the gateway and watch for config changes.
    pub async fn start(config_path: &str) -> anyhow::Result<()> {
        let config_str = std::fs::read_to_string(config_path)?;
        let config: GatewayConfig = match serde_yaml::from_str(&config_str) {
            Ok(c) => c,
            Err(e) => {
                let msg = format!("YAML Error in Gateway Config {}: {}", config_path, e);
                tracing::error!("{}", msg);
                return Err(anyhow::anyhow!(msg));
            }
        };

        // Validate: At least one route with a non-empty upstream
        if config.routes.is_empty() {
            return Err(anyhow::anyhow!("Gateway Config Error: 'routes' must contain at least one entry."));
        }

        for (i, route) in config.routes.iter().enumerate() {
            if route.upstream.trim().is_empty() {
                return Err(anyhow::anyhow!("Gateway Config Error (Route #{}): 'upstream' URL cannot be empty.", i + 1));
            }
        }

        let initial_pipeline = Self::build_pipeline(&config)?;
        let shared_pipeline = Arc::new(ArcSwap::from_pointee(initial_pipeline));
        let listen_addr = config.listen_addr.clone();

        // 1. Setup Hot Reload Watcher
        let pipeline_for_watcher = Arc::clone(&shared_pipeline);
        let path_str = config_path.to_string();
        
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(event) = res {
                if event.kind.is_modify() {
                    tracing::info!("Config change detected, reloading...");
                    if let Ok(new_str) = std::fs::read_to_string(&path_str) {
                        if let Ok(new_cfg) = serde_yaml::from_str::<GatewayConfig>(&new_str) {
                            if let Ok(new_pipe) = Self::build_pipeline(&new_cfg) {
                                pipeline_for_watcher.store(Arc::new(new_pipe));
                                tracing::info!("Gateway pipeline updated successfully!");
                            } else {
                                tracing::error!("Failed to rebuild pipeline from new config");
                            }
                        } else {
                            tracing::error!("Failed to parse new config schema");
                        }
                    }
                }
            }
        })?;

        watcher.watch(Path::new(config_path), RecursiveMode::NonRecursive)?;

        // 2. Build the router based on static routes in config
        // NOTE: In this basic implementation, adding NEW routes requires a restart,
        // but changes to transformations and auth are atomic and instant.
        let mut router = Router::new();
        for route in config.routes {
            let method = match route.method.to_uppercase().as_str() {
                "GET" => HttpMethod::GET,
                "POST" => HttpMethod::POST,
                "PUT" => HttpMethod::PUT,
                "DELETE" => HttpMethod::DELETE,
                _ => HttpMethod::GET,
            };

            let pipeline_ref = Arc::clone(&shared_pipeline);
            let upstream = route.upstream.clone();

            router.add_http(method, &route.path, Arc::new(move |req| {
                let pipeline = pipeline_ref.load_full();
                let upstream_url = upstream.clone();
                async move {
                    let mut ctx = Context::new(req);
                    ctx.upstream_url = Some(upstream_url);
                    pipeline.execute(ctx).await
                }.boxed()
            }));
        }

        // 3. Start Server
        let server_config = ServerConfig::default();
        let server = HttpServer::new(router, server_config);
        
        tracing::info!("Pulsar Gateway [Phase 15] listening on http://{}", listen_addr);
        server.run(&listen_addr).await?;

        Ok(())
    }

    /// Build a new pipeline from a config.
    fn build_pipeline(config: &GatewayConfig) -> anyhow::Result<Pipeline> {
        let mut plugins: Vec<Box<dyn Plugin>> = vec![];

        // 1. Auth Plugin
        if let Some(auth_cfg) = &config.auth {
            plugins.push(Box::new(AuthPlugin::new(auth_cfg.clone())));
        }

        // 2. Rate Limit Plugin (Phase 16)
        if let Some(rl_cfg) = &config.rate_limit {
            plugins.push(Box::new(RateLimitPlugin::new(rl_cfg.clone())));
        }

        // 3. Transform Plugin
        if let Some(trans_cfg) = &config.transform {
            plugins.push(Box::new(TransformPlugin::new(trans_cfg.clone())));
        }

        // 4. Final Resilient Forwarder (Phase 17)
        let upstream_cfg = config.upstream.clone().unwrap_or_default();
        plugins.push(Box::new(ResilientPassthroughPlugin::new(upstream_cfg)));

        Ok(Pipeline::new(plugins))
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
            peer_addr: "127.0.0.1:12345".parse().unwrap(),
        };
        Context::new(req)
    }

    #[tokio::test]
    async fn test_transform_plugin_full_suite() {
        let yaml = r#"
prefix_strip: "/api/v1"
inject_response_headers:
  Access-Control-Allow-Origin: "*"
  X-Test: "Pulsar"
strip_headers:
  - "x-internal-id"
body_transformations:
  "old_key": "new_key"
response_body_strip:
  - "internal_field"
"#;
        let plugin = TransformPlugin::from_config(yaml).unwrap();
        
        // Use a capture structure to inspect what Request the next plugin sees
        struct CapturePlugin;
        impl Plugin for CapturePlugin {
            fn call<'a>(&'a self, ctx: &'a mut Context, _next: Next) -> BoxFuture<'a, GatewayResponse> {
                Box::pin(async move {
                    // Check Request Injections
                    assert!(ctx.request.headers.contains_key("x-request-id"));
                    assert!(ctx.request.headers.contains_key("x-forwarded-for"));
                    
                    // Check Path Rewriting
                    assert_eq!(ctx.request.path, "/users");

                    // Check Body Transformation
                    let body: serde_json::Value = serde_json::from_slice(&ctx.request.body).unwrap();
                    assert!(body.get("new_key").is_some());
                    assert!(body.get("old_key").is_none());

                    // Return a dummy response with an internal header to be stripped
                    let mut res = GatewayResponse::new(200);
                    res.headers.insert("x-internal-id".to_string(), "secret".to_string());
                    res.body = r#"{"ok":true,"internal_field":"leak"}"#.as_bytes().to_vec();
                    res
                })
            }
        }

        let pipeline = Pipeline::new(vec![Box::new(plugin), Box::new(CapturePlugin)]);

        let ctx = mock_context("/api/v1/users", Some(r#"{"old_key":"value"}"#));
        let res = pipeline.execute(ctx).await;

        // 1. Verify Request headers were injected (checked inside CapturePlugin)
        
        // 2. Verify Response Header Injection
        assert_eq!(res.headers.get("access-control-allow-origin").unwrap(), "*");
        assert_eq!(res.headers.get("x-test").unwrap(), "Pulsar");

        // 3. Verify Response Header Stripping
        assert!(!res.headers.contains_key("x-internal-id"));

        // 4. Verify Response Body Stripping
        let body_val: serde_json::Value = serde_json::from_slice(&res.body).unwrap();
        assert!(body_val.get("internal_field").is_none());
        assert_eq!(body_val.get("ok").unwrap(), true);
        
        tracing::info!("Phase 14 transformation test suite passed!");
    }

    #[tokio::test]
    async fn test_rate_limit_integration() {
        let mut limits = HashMap::new();
        limits.insert("/api".to_string(), 1); // Only 1 request allowed per 60s
        
        let rl_config = RateLimitConfig {
            api_key_header: "x-api-key".to_string(),
            limits,
        };
        
        let plugin = RateLimitPlugin::new(rl_config);
        
        // Mock a final forwarder that always returns 200
        struct SuccessPlugin;
        impl Plugin for SuccessPlugin {
            fn call<'a>(&'a self, _ctx: &'a mut Context, _next: Next) -> BoxFuture<'a, GatewayResponse> {
                Box::pin(async { GatewayResponse::new(200) })
            }
        }

        let pipeline = Pipeline::new(vec![Box::new(plugin), Box::new(SuccessPlugin)]);

        // First request: Should be allowed (200)
        let ctx1 = mock_context("/api/users", None);
        let res1 = pipeline.execute(ctx1).await;
        assert_eq!(res1.status, 200);

        // Second request from same IP (127.0.0.1): Should be rejected (429)
        let ctx2 = mock_context("/api/users", None);
        let res2 = pipeline.execute(ctx2).await;
        assert_eq!(res2.status, 429);
        assert!(res2.headers.contains_key("x-ratelimit-limit"));
        assert!(res2.headers.contains_key("retry-after"));
        
        tracing::info!("Phase 16 Rate Limit integration test passed!");
    }
}
