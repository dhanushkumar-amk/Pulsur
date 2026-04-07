use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use axum::body::{to_bytes, Body};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::header::{
    ACCEPT, ACCEPT_ENCODING, AGE, CACHE_CONTROL, CONNECTION, CONTENT_LENGTH, CONTENT_TYPE, HOST,
    TRANSFER_ENCODING, UPGRADE, VIA,
};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, Request, Response, StatusCode, Uri};
use axum::response::IntoResponse;
use axum::routing::{any, post};
use axum::{Json, Router};
use futures_util::{SinkExt, StreamExt};
use moka::sync::Cache;
use notify::{RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::Message as TungsteniteMessage;
use tracing::{error, info, warn};

const DEFAULT_CACHE_MAX_ENTRIES: u64 = 1_024;
const DEFAULT_CACHE_TTL_SECS: u64 = 30;
const PROXY_VIA_HEADER: &str = "ferrum/1.0";
const HOP_BY_HOP_HEADERS: [&str; 8] = [
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

#[derive(Debug, Error)]
pub enum ProxyError {
    #[error("proxy config must contain at least one location")]
    MissingLocations,
    #[error("location `{0}` must define exactly one of upstream/static_dir/websocket")]
    InvalidLocation(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("yaml error: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("notify error: {0}")]
    Notify(#[from] notify::Error),
    #[error("request error: {0}")]
    Request(#[from] reqwest::Error),
    #[error("websocket error: {0}")]
    WebSocket(Box<tokio_tungstenite::tungstenite::Error>),
    #[error("http error: {0}")]
    Http(#[from] http::Error),
    #[error("invalid header value: {0}")]
    InvalidHeaderValue(#[from] axum::http::header::InvalidHeaderValue),
    #[error("invalid uri: {0}")]
    InvalidUri(#[from] axum::http::uri::InvalidUri),
}

impl From<tokio_tungstenite::tungstenite::Error> for ProxyError {
    fn from(value: tokio_tungstenite::tungstenite::Error) -> Self {
        Self::WebSocket(Box::new(value))
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ReverseProxyConfig {
    pub listen_addr: String,
    pub locations: Vec<LocationConfig>,
    #[serde(default)]
    pub cache: ProxyCacheConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LocationConfig {
    pub path: String,
    pub upstream: Option<String>,
    pub static_dir: Option<String>,
    pub websocket: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProxyCacheConfig {
    #[serde(default = "default_cache_max_entries")]
    pub max_entries: u64,
    #[serde(default = "default_cache_ttl_secs")]
    pub default_ttl_secs: u64,
}

impl Default for ProxyCacheConfig {
    fn default() -> Self {
        Self {
            max_entries: default_cache_max_entries(),
            default_ttl_secs: default_cache_ttl_secs(),
        }
    }
}

fn default_cache_max_entries() -> u64 {
    DEFAULT_CACHE_MAX_ENTRIES
}

fn default_cache_ttl_secs() -> u64 {
    DEFAULT_CACHE_TTL_SECS
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum LocationTarget {
    Upstream(String),
    StaticDir(PathBuf),
    WebSocket(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LocationRoute {
    path: String,
    target: LocationTarget,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CacheMetricsView {
    pub hits: u64,
    pub misses: u64,
    pub invalidations: u64,
    pub hit_rate: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CacheInvalidationResponse {
    pub invalidated_keys: usize,
    pub path: String,
}

#[derive(Debug, Clone)]
struct CachedResponse {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    cached_at: Instant,
    expires_at: Instant,
}

#[derive(Debug, Default)]
struct CacheCounters {
    hits: AtomicU64,
    misses: AtomicU64,
    invalidations: AtomicU64,
}

#[derive(Debug, Clone)]
pub struct ReverseProxy {
    routes: Arc<Vec<LocationRoute>>,
    client: reqwest::Client,
    cache: Cache<String, CachedResponse>,
    cache_ttl: Duration,
    path_index: Arc<Mutex<HashMap<String, Vec<String>>>>,
    counters: Arc<CacheCounters>,
}

impl ReverseProxy {
    pub fn from_config(config: &ReverseProxyConfig) -> Result<Self, ProxyError> {
        if config.locations.is_empty() {
            return Err(ProxyError::MissingLocations);
        }

        let mut routes = Vec::with_capacity(config.locations.len());
        for location in &config.locations {
            routes.push(LocationRoute::from_config(location)?);
        }

        Ok(Self {
            routes: Arc::new(routes),
            client: reqwest::Client::new(),
            cache: Cache::builder().max_capacity(config.cache.max_entries).build(),
            cache_ttl: Duration::from_secs(config.cache.default_ttl_secs),
            path_index: Arc::new(Mutex::new(HashMap::new())),
            counters: Arc::new(CacheCounters::default()),
        })
    }

    pub fn load_config_file(path: impl AsRef<Path>) -> Result<ReverseProxyConfig, ProxyError> {
        let content = std::fs::read_to_string(path)?;
        Self::load_config_str(&content)
    }

    pub fn load_config_str(content: &str) -> Result<ReverseProxyConfig, ProxyError> {
        let config: ReverseProxyConfig = serde_yaml::from_str(content)?;
        for location in &config.locations {
            let _ = LocationRoute::from_config(location)?;
        }
        Ok(config)
    }

    fn match_route(&self, path: &str, websocket: bool) -> Option<LocationRoute> {
        self.routes
            .iter()
            .filter(|route| path.starts_with(&route.path))
            .filter(|route| match (&route.target, websocket) {
                (LocationTarget::WebSocket(_), true) => true,
                (LocationTarget::WebSocket(_), false) => false,
                (_, true) => false,
                _ => true,
            })
            .max_by_key(|route| route.path.len())
            .cloned()
    }

    pub async fn handle_http_request(&self, request: Request<Body>) -> Response<Body> {
        let path = request.uri().path().to_string();
        let Some(route) = self.match_route(&path, false) else {
            return text_response(StatusCode::NOT_FOUND, "No matching proxy location");
        };

        match route.target {
            LocationTarget::StaticDir(dir) => {
                self.serve_static_file(&route.path, &dir, request.uri()).await
            }
            LocationTarget::Upstream(upstream) => self.proxy_http_request(&upstream, request).await,
            LocationTarget::WebSocket(_) => {
                text_response(StatusCode::BAD_REQUEST, "WebSocket location requires Upgrade")
            }
        }
    }

    pub async fn handle_websocket_upgrade(
        &self,
        websocket: WebSocketUpgrade,
        request: Request<Body>,
    ) -> Response<Body> {
        let path = request.uri().path().to_string();
        let Some(route) = self.match_route(&path, true) else {
            return text_response(StatusCode::NOT_FOUND, "No matching websocket location");
        };

        match route.target {
            LocationTarget::WebSocket(upstream) => {
                let suffix = request
                    .uri()
                    .path_and_query()
                    .map(|value| value.as_str())
                    .unwrap_or_else(|| request.uri().path());
                let upstream_url = format!("{}{}", upstream.trim_end_matches('/'), suffix);
                let headers = request.headers().clone();
                websocket
                    .on_upgrade(move |socket| async move {
                        if let Err(err) = proxy_websocket(socket, upstream_url, headers).await {
                            warn!("websocket proxy failed: {}", err);
                        }
                    })
                    .into_response()
            }
            _ => text_response(StatusCode::BAD_REQUEST, "Location is not a websocket upstream"),
        }
    }

    pub fn cache_metrics(&self) -> CacheMetricsView {
        let hits = self.counters.hits.load(Ordering::Relaxed);
        let misses = self.counters.misses.load(Ordering::Relaxed);
        let invalidations = self.counters.invalidations.load(Ordering::Relaxed);
        let hit_rate = if hits + misses == 0 {
            0.0
        } else {
            hits as f64 / (hits + misses) as f64
        };

        CacheMetricsView {
            hits,
            misses,
            invalidations,
            hit_rate,
        }
    }

    pub fn invalidate_path(&self, path: &str) -> usize {
        let keys = self
            .path_index
            .lock()
            .expect("path index poisoned")
            .remove(path)
            .unwrap_or_default();

        let count = keys.len();
        for key in keys {
            self.cache.invalidate(&key);
        }

        if count > 0 {
            self.counters
                .invalidations
                .fetch_add(count as u64, Ordering::Relaxed);
        }

        count
    }

    async fn serve_static_file(&self, prefix: &str, root: &Path, uri: &Uri) -> Response<Body> {
        let Some(full_path) = resolve_static_path(root, prefix, uri.path()) else {
            return text_response(StatusCode::FORBIDDEN, "Invalid static file path");
        };

        match tokio::fs::read(&full_path).await {
            Ok(bytes) => {
                let mime = mime_guess::from_path(&full_path).first_or_octet_stream();
                let mut response = Response::new(Body::from(bytes));
                *response.status_mut() = StatusCode::OK;
                response.headers_mut().insert(
                    CONTENT_TYPE,
                    HeaderValue::from_str(mime.as_ref())
                        .unwrap_or(HeaderValue::from_static("application/octet-stream")),
                );
                response
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                text_response(StatusCode::NOT_FOUND, "Static file not found")
            }
            Err(err) => {
                error!("static file error: {}", err);
                text_response(StatusCode::INTERNAL_SERVER_ERROR, "Static file read failed")
            }
        }
    }

    async fn proxy_http_request(&self, upstream: &str, request: Request<Body>) -> Response<Body> {
        let method = request.method().clone();
        let request_path = request
            .uri()
            .path_and_query()
            .map(|value| value.as_str().to_string())
            .unwrap_or_else(|| request.uri().path().to_string());
        let cache_path = request.uri().path().to_string();
        let cache_key = build_cache_key(&method, request.uri(), request.headers());

        if is_cacheable_method(&method) {
            if let Some(cached) = self.cache.get(&cache_key) {
                if cached.expires_at > Instant::now() {
                    self.counters.hits.fetch_add(1, Ordering::Relaxed);
                    return cached_response_to_http(cached);
                }
                self.cache.invalidate(&cache_key);
            }
            self.counters.misses.fetch_add(1, Ordering::Relaxed);
        }

        let target_url = format!("{}{}", upstream.trim_end_matches('/'), request_path);
        let (parts, body) = request.into_parts();
        let body_bytes = match to_bytes(body, usize::MAX).await {
            Ok(bytes) => bytes.to_vec(),
            Err(err) => {
                error!("request body read failed: {}", err);
                return text_response(StatusCode::BAD_REQUEST, "Failed to read request body");
            }
        };

        let mut builder = self.client.request(method.clone(), target_url);
        for (name, value) in filter_forward_headers(&parts.headers, false) {
            builder = builder.header(name, value);
        }

        let via_value = append_via(parts.headers.get(VIA));
        builder = builder.header(VIA, via_value);
        builder = builder.body(body_bytes);

        let upstream_response = match builder.send().await {
            Ok(response) => response,
            Err(err) => {
                error!("upstream request failed: {}", err);
                return text_response(StatusCode::BAD_GATEWAY, "Upstream request failed");
            }
        };

        let status = upstream_response.status();
        let headers = upstream_response.headers().clone();
        let body_bytes = match upstream_response.bytes().await {
            Ok(bytes) => bytes.to_vec(),
            Err(err) => {
                error!("upstream body read failed: {}", err);
                return text_response(StatusCode::BAD_GATEWAY, "Failed to read upstream response");
            }
        };

        let mut response = response_from_parts(status, &headers, body_bytes.clone());
        if let Some(cached) =
            maybe_cache_response(&method, status, &headers, &body_bytes, self.cache_ttl)
        {
            self.index_cache_key(&cache_path, cache_key.clone());
            self.cache.insert(cache_key, cached);
        }

        response
            .headers_mut()
            .insert(VIA, HeaderValue::from_static(PROXY_VIA_HEADER));
        response
    }

    fn index_cache_key(&self, path: &str, key: String) {
        let mut index = self.path_index.lock().expect("path index poisoned");
        index.entry(path.to_string()).or_default().push(key);
    }
}

impl LocationRoute {
    fn from_config(config: &LocationConfig) -> Result<Self, ProxyError> {
        let targets = [
            config.upstream.is_some(),
            config.static_dir.is_some(),
            config.websocket.is_some(),
        ]
        .into_iter()
        .filter(|selected| *selected)
        .count();

        if targets != 1 {
            return Err(ProxyError::InvalidLocation(config.path.clone()));
        }

        let target = if let Some(upstream) = &config.upstream {
            LocationTarget::Upstream(upstream.clone())
        } else if let Some(dir) = &config.static_dir {
            LocationTarget::StaticDir(PathBuf::from(dir))
        } else {
            LocationTarget::WebSocket(config.websocket.clone().unwrap_or_default())
        };

        Ok(Self {
            path: config.path.clone(),
            target,
        })
    }
}

#[derive(Debug, Clone)]
pub struct HotReloadProxy {
    config_path: String,
    runtime: Arc<ArcSwap<ReverseProxy>>,
}

impl HotReloadProxy {
    pub async fn start(config_path: &str) -> Result<(), ProxyError> {
        let config = ReverseProxy::load_config_file(config_path)?;
        let runtime = Arc::new(ArcSwap::from_pointee(ReverseProxy::from_config(&config)?));

        let app = Self::router(Arc::clone(&runtime));
        let listener = TcpListener::bind(&config.listen_addr).await?;

        let mut watcher = Self::watcher(config_path.to_string(), Arc::clone(&runtime))?;
        watcher.watch(Path::new(config_path), RecursiveMode::NonRecursive)?;

        info!("Ferrum proxy listening on http://{}", config.listen_addr);
        axum::serve(listener, app).await?;
        Ok(())
    }

    pub fn new(config_path: impl Into<String>, runtime: Arc<ArcSwap<ReverseProxy>>) -> Self {
        Self {
            config_path: config_path.into(),
            runtime,
        }
    }

    pub fn config_path(&self) -> &str {
        &self.config_path
    }

    pub fn runtime(&self) -> Arc<ArcSwap<ReverseProxy>> {
        Arc::clone(&self.runtime)
    }

    pub fn router(runtime: Arc<ArcSwap<ReverseProxy>>) -> Router {
        Router::new()
            .route("/proxy/cache/invalidate", post(invalidate_cache))
            .route("/*path", any(proxy_entry))
            .route("/", any(proxy_entry))
            .with_state(runtime)
    }

    fn watcher(
        config_path: String,
        runtime: Arc<ArcSwap<ReverseProxy>>,
    ) -> Result<impl Watcher, ProxyError> {
        let watcher = notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
            let Ok(event) = result else {
                return;
            };

            if !event.kind.is_modify() {
                return;
            }

            match ReverseProxy::load_config_file(&config_path)
                .and_then(|config| ReverseProxy::from_config(&config))
            {
                Ok(proxy) => {
                    runtime.store(Arc::new(proxy));
                    info!("proxy configuration reloaded");
                }
                Err(err) => error!("proxy configuration reload failed: {}", err),
            }
        })?;
        Ok(watcher)
    }
}

async fn proxy_entry(
    State(runtime): State<Arc<ArcSwap<ReverseProxy>>>,
    websocket: Option<WebSocketUpgrade>,
    request: Request<Body>,
) -> Response<Body> {
    let proxy = runtime.load_full();
    let wants_websocket = request
        .headers()
        .get(UPGRADE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("websocket"));

    if wants_websocket {
        if let Some(upgrade) = websocket {
            return proxy.handle_websocket_upgrade(upgrade, request).await;
        }
        return text_response(StatusCode::BAD_REQUEST, "Missing websocket upgrade context");
    }

    proxy.handle_http_request(request).await
}

#[derive(Debug, Deserialize)]
struct CacheInvalidateQuery {
    path: String,
}

async fn invalidate_cache(
    State(runtime): State<Arc<ArcSwap<ReverseProxy>>>,
    Query(query): Query<CacheInvalidateQuery>,
) -> Json<CacheInvalidationResponse> {
    let proxy = runtime.load_full();
    Json(CacheInvalidationResponse {
        invalidated_keys: proxy.invalidate_path(&query.path),
        path: query.path,
    })
}

fn filter_forward_headers(headers: &HeaderMap, websocket: bool) -> Vec<(HeaderName, HeaderValue)> {
    headers
        .iter()
        .filter_map(|(name, value)| {
            let lower = name.as_str().to_ascii_lowercase();
            let blocked = HOP_BY_HOP_HEADERS.contains(&lower.as_str())
                || name == HOST
                || (!websocket && name == UPGRADE)
                || (!websocket && name == CONNECTION)
                || name == TRANSFER_ENCODING;
            if blocked {
                None
            } else {
                Some((name.clone(), value.clone()))
            }
        })
        .collect()
}

fn append_via(existing: Option<&HeaderValue>) -> String {
    existing
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .map(|value| format!("{value}, {PROXY_VIA_HEADER}"))
        .unwrap_or_else(|| PROXY_VIA_HEADER.to_string())
}

fn build_cache_key(method: &Method, uri: &Uri, headers: &HeaderMap) -> String {
    let mut hasher = Sha256::new();
    hasher.update(method.as_str().as_bytes());
    hasher.update(
        uri.path_and_query()
            .map(|value| value.as_str())
            .unwrap_or(uri.path())
            .as_bytes(),
    );

    for header in [ACCEPT, ACCEPT_ENCODING] {
        if let Some(value) = headers.get(&header).and_then(|value| value.to_str().ok()) {
            hasher.update(header.as_str().as_bytes());
            hasher.update(value.as_bytes());
        }
    }

    format!("{:x}", hasher.finalize())
}

fn maybe_cache_response(
    method: &Method,
    status: StatusCode,
    headers: &HeaderMap,
    body: &[u8],
    default_ttl: Duration,
) -> Option<CachedResponse> {
    if !is_cacheable_method(method) || !status.is_success() {
        return None;
    }

    let cache_control = headers
        .get(CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();

    if cache_control.contains("no-store") || cache_control.contains("no-cache") {
        return None;
    }

    let ttl = parse_max_age(&cache_control)
        .map(Duration::from_secs)
        .unwrap_or(default_ttl);
    let now = Instant::now();

    let stored_headers = headers
        .iter()
        .filter(|(name, _)| !HOP_BY_HOP_HEADERS.contains(&name.as_str()))
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_string(), value.to_string()))
        })
        .collect();

    Some(CachedResponse {
        status: status.as_u16(),
        headers: stored_headers,
        body: body.to_vec(),
        cached_at: now,
        expires_at: now + ttl,
    })
}

fn cached_response_to_http(cached: CachedResponse) -> Response<Body> {
    let mut response = Response::new(Body::from(cached.body));
    *response.status_mut() = StatusCode::from_u16(cached.status).unwrap_or(StatusCode::OK);

    for (name, value) in cached.headers {
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_str(&value),
        ) {
            response.headers_mut().insert(name, value);
        }
    }

    let age = cached.cached_at.elapsed().as_secs();
    if let Ok(value) = HeaderValue::from_str(&age.to_string()) {
        response.headers_mut().insert(AGE, value);
    }

    response
}

fn response_from_parts(status: StatusCode, headers: &HeaderMap, body: Vec<u8>) -> Response<Body> {
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = status;

    for (name, value) in headers {
        let lower = name.as_str().to_ascii_lowercase();
        if HOP_BY_HOP_HEADERS.contains(&lower.as_str()) || name == CONTENT_LENGTH {
            continue;
        }
        response.headers_mut().insert(name.clone(), value.clone());
    }

    response
}

fn parse_max_age(cache_control: &str) -> Option<u64> {
    cache_control
        .split(',')
        .map(str::trim)
        .find_map(|directive| directive.strip_prefix("max-age=")?.parse::<u64>().ok())
}

fn is_cacheable_method(method: &Method) -> bool {
    matches!(*method, Method::GET | Method::HEAD)
}

fn resolve_static_path(root: &Path, prefix: &str, request_path: &str) -> Option<PathBuf> {
    let suffix = request_path
        .strip_prefix(prefix)
        .unwrap_or(request_path)
        .trim_start_matches('/');

    let mut resolved = root.to_path_buf();
    for component in Path::new(suffix).components() {
        match component {
            Component::Normal(value) => resolved.push(value),
            Component::CurDir => {}
            _ => return None,
        }
    }

    Some(resolved)
}

async fn proxy_websocket(
    downstream: WebSocket,
    upstream_url: String,
    headers: HeaderMap,
) -> Result<(), ProxyError> {
    let mut upstream_request = upstream_url.into_client_request()?;
    for (name, value) in filter_forward_headers(&headers, true) {
        upstream_request.headers_mut().insert(name, value);
    }
    upstream_request
        .headers_mut()
        .insert(VIA, HeaderValue::from_static(PROXY_VIA_HEADER));

    let (upstream_socket, _) = tokio_tungstenite::connect_async(upstream_request).await?;
    let (mut downstream_sender, mut downstream_receiver) = downstream.split();
    let (mut upstream_sender, mut upstream_receiver) = upstream_socket.split();

    let client_to_upstream = async {
        while let Some(message) = downstream_receiver.next().await {
            let message = message.map_err(|err| std::io::Error::other(err.to_string()))?;
            upstream_sender.send(axum_to_tungstenite(message)).await?;
        }
        Ok::<(), ProxyError>(())
    };

    let upstream_to_client = async {
        while let Some(message) = upstream_receiver.next().await {
            let message = message?;
            downstream_sender
                .send(tungstenite_to_axum(message))
                .await
                .map_err(|err| std::io::Error::other(err.to_string()))?;
        }
        Ok::<(), ProxyError>(())
    };

    tokio::select! {
        result = client_to_upstream => result?,
        result = upstream_to_client => result?,
    }

    Ok(())
}

fn axum_to_tungstenite(message: Message) -> TungsteniteMessage {
    match message {
        Message::Text(text) => TungsteniteMessage::Text(text.to_string()),
        Message::Binary(bytes) => TungsteniteMessage::Binary(bytes.to_vec()),
        Message::Ping(bytes) => TungsteniteMessage::Ping(bytes.to_vec()),
        Message::Pong(bytes) => TungsteniteMessage::Pong(bytes.to_vec()),
        Message::Close(frame) => TungsteniteMessage::Close(frame.map(|frame| {
            tokio_tungstenite::tungstenite::protocol::CloseFrame {
                code: frame.code.into(),
                reason: frame.reason.to_string().into(),
            }
        })),
    }
}

fn tungstenite_to_axum(message: TungsteniteMessage) -> Message {
    match message {
        TungsteniteMessage::Text(text) => Message::Text(text),
        TungsteniteMessage::Binary(bytes) => Message::Binary(bytes),
        TungsteniteMessage::Ping(bytes) => Message::Ping(bytes),
        TungsteniteMessage::Pong(bytes) => Message::Pong(bytes),
        TungsteniteMessage::Close(frame) => Message::Close(frame.map(|frame| {
            axum::extract::ws::CloseFrame {
                code: frame.code.into(),
                reason: frame.reason.to_string().into(),
            }
        })),
        TungsteniteMessage::Frame(frame) => Message::Binary(frame.into_data()),
    }
}

fn text_response(status: StatusCode, message: &str) -> Response<Body> {
    let mut response = Response::new(Body::from(message.to_string()));
    *response.status_mut() = status;
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::get;
    use serde_json::json;
    use tempfile::TempDir;
    use tower::util::ServiceExt;

    fn sample_config(static_dir: &Path) -> ReverseProxyConfig {
        ReverseProxyConfig {
            listen_addr: "127.0.0.1:0".to_string(),
            locations: vec![
                LocationConfig {
                    path: "/api/".to_string(),
                    upstream: Some("http://127.0.0.1:4010".to_string()),
                    static_dir: None,
                    websocket: None,
                },
                LocationConfig {
                    path: "/static/".to_string(),
                    upstream: None,
                    static_dir: Some(static_dir.display().to_string()),
                    websocket: None,
                },
                LocationConfig {
                    path: "/ws/".to_string(),
                    upstream: None,
                    static_dir: None,
                    websocket: Some("ws://127.0.0.1:4020".to_string()),
                },
            ],
            cache: ProxyCacheConfig {
                max_entries: 100,
                default_ttl_secs: 60,
            },
        }
    }

    async fn spawn_upstream() -> (String, Arc<AtomicU64>) {
        let hits = Arc::new(AtomicU64::new(0));
        let state = Arc::clone(&hits);
        let app = Router::new()
            .route(
                "/api/hello",
                get(
                    move |headers: HeaderMap, State(state): State<Arc<AtomicU64>>| async move {
                        state.fetch_add(1, Ordering::Relaxed);
                        let via = headers
                            .get(VIA)
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default()
                            .to_string();
                        let connection = headers.get(CONNECTION).is_some();
                        Json(json!({
                            "via": via,
                            "has_connection": connection,
                        }))
                    },
                ),
            )
            .with_state(state);

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind upstream");
        let addr = listener.local_addr().expect("local addr");
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve upstream");
        });
        (format!("http://{}", addr), hits)
    }

    #[test]
    fn longest_prefix_match_prefers_more_specific_route() {
        let config = ReverseProxyConfig {
            listen_addr: "127.0.0.1:0".to_string(),
            locations: vec![
                LocationConfig {
                    path: "/api/".to_string(),
                    upstream: Some("http://example.com".to_string()),
                    static_dir: None,
                    websocket: None,
                },
                LocationConfig {
                    path: "/api/internal/".to_string(),
                    upstream: Some("http://internal.example.com".to_string()),
                    static_dir: None,
                    websocket: None,
                },
            ],
            cache: ProxyCacheConfig::default(),
        };

        let proxy = ReverseProxy::from_config(&config).expect("proxy should build");
        let route = proxy
            .match_route("/api/internal/users", false)
            .expect("route");
        assert_eq!(route.path, "/api/internal/");
    }

    #[tokio::test]
    async fn serves_static_files_from_matching_location() {
        let temp_dir = TempDir::new().expect("temp dir");
        let asset = temp_dir.path().join("hello.txt");
        tokio::fs::write(&asset, b"hello proxy")
            .await
            .expect("write asset");

        let proxy = ReverseProxy::from_config(&sample_config(temp_dir.path())).expect("proxy");
        let request = Request::builder()
            .uri("/static/hello.txt")
            .body(Body::empty())
            .expect("request");

        let response = proxy.handle_http_request(request).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        assert_eq!(body.as_ref(), b"hello proxy");
    }

    #[tokio::test]
    async fn proxies_http_and_strips_hop_by_hop_headers() {
        let temp_dir = TempDir::new().expect("temp dir");
        let (upstream, hits) = spawn_upstream().await;
        let mut config = sample_config(temp_dir.path());
        config.locations[0].upstream = Some(upstream);
        let proxy = ReverseProxy::from_config(&config).expect("proxy");

        let request = Request::builder()
            .method(Method::GET)
            .uri("/api/hello")
            .header(CONNECTION, "keep-alive")
            .body(Body::empty())
            .expect("request");

        let response = proxy.handle_http_request(request).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let payload: serde_json::Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(payload["via"], PROXY_VIA_HEADER);
        assert_eq!(payload["has_connection"], false);
        assert_eq!(hits.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn caches_get_responses_and_reports_hits() {
        let temp_dir = TempDir::new().expect("temp dir");
        let (upstream, hits) = spawn_upstream().await;
        let mut config = sample_config(temp_dir.path());
        config.locations[0].upstream = Some(upstream);
        let proxy = ReverseProxy::from_config(&config).expect("proxy");

        for _ in 0..2 {
            let request = Request::builder()
                .method(Method::GET)
                .uri("/api/hello")
                .body(Body::empty())
                .expect("request");
            let response = proxy.handle_http_request(request).await;
            assert_eq!(response.status(), StatusCode::OK);
        }

        assert_eq!(hits.load(Ordering::Relaxed), 1);
        let metrics = proxy.cache_metrics();
        assert_eq!(metrics.hits, 1);
        assert_eq!(metrics.misses, 1);
    }

    #[tokio::test]
    async fn invalidation_endpoint_clears_cached_entries() {
        let temp_dir = TempDir::new().expect("temp dir");
        let (upstream, hits) = spawn_upstream().await;
        let mut config = sample_config(temp_dir.path());
        config.locations[0].upstream = Some(upstream);
        let runtime = Arc::new(ArcSwap::from_pointee(
            ReverseProxy::from_config(&config).expect("proxy"),
        ));
        let app = HotReloadProxy::router(Arc::clone(&runtime));

        let first = Request::builder()
            .method(Method::GET)
            .uri("/api/hello")
            .body(Body::empty())
            .expect("request");
        let _ = app.clone().oneshot(first).await.expect("first response");

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/proxy/cache/invalidate?path=/api/hello")
                    .body(Body::empty())
                    .expect("invalidate request"),
            )
            .await
            .expect("invalidate response");
        assert_eq!(response.status(), StatusCode::OK);

        let second = Request::builder()
            .method(Method::GET)
            .uri("/api/hello")
            .body(Body::empty())
            .expect("request");
        let _ = HotReloadProxy::router(runtime)
            .oneshot(second)
            .await
            .expect("second response");

        assert_eq!(hits.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn parses_hot_reload_yaml_config() {
        let yaml = r#"
listen_addr: 127.0.0.1:8080
cache:
  max_entries: 256
  default_ttl_secs: 45
locations:
  - path: /api/
    upstream: http://localhost:4000
  - path: /static/
    static_dir: ./public
  - path: /ws/
    websocket: ws://localhost:4001
"#;

        let config = ReverseProxy::load_config_str(yaml).expect("config should parse");
        assert_eq!(config.locations.len(), 3);
        assert_eq!(config.cache.max_entries, 256);
    }
}
