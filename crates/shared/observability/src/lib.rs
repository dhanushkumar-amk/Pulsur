use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, OnceLock, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_stream::stream;
use axum::extract::State;
use axum::http::HeaderValue;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use metrics::{counter, describe_counter, describe_gauge, describe_histogram, gauge, histogram};
use metrics_exporter_prometheus::{BuildError, Matcher, PrometheusBuilder, PrometheusHandle};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::net::TcpListener;

#[cfg(test)]
use std::sync::Mutex;

const SNAPSHOT_WINDOW: Duration = Duration::from_secs(60);
const STREAM_INTERVAL: Duration = Duration::from_secs(1);
const REQUEST_DURATION_BUCKETS: &[f64] = &[1.0, 5.0, 10.0, 50.0, 100.0, 500.0, 1000.0];

#[derive(Debug, Error)]
pub enum ObservabilityError {
    #[error("prometheus setup failed: {0}")]
    Prometheus(#[from] BuildError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("snapshot serialization failed: {0}")]
    SnapshotSerialization(#[from] serde_json::Error),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComponentState {
    Healthy,
    Degraded,
    Down,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentHealth {
    pub component: String,
    pub state: ComponentState,
    pub detail: String,
    pub updated_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricsSnapshot {
    pub timestamp_ms: u64,
    pub request_count: u64,
    pub req_per_sec: f64,
    pub request_duration_p99_ms: f64,
    pub error_count: u64,
    pub error_rate: f64,
    pub active_connections: i64,
    pub queue_depth: i64,
    pub cache_hit_rate: f64,
    pub component_health: Vec<ComponentHealth>,
}

#[derive(Debug, Default)]
struct MutableState {
    request_count: u64,
    error_count: u64,
    request_times: VecDeque<Instant>,
    error_times: VecDeque<Instant>,
    latency_samples: VecDeque<(Instant, f64)>,
    active_connections: HashMap<String, i64>,
    queue_depths: HashMap<String, i64>,
    cache_hit_rates: HashMap<String, f64>,
    component_health: HashMap<String, ComponentHealth>,
}

#[derive(Clone)]
pub struct ObservabilityAgent {
    handle: PrometheusHandle,
    state: Arc<RwLock<MutableState>>,
}

static GLOBAL_AGENT: OnceLock<Arc<ObservabilityAgent>> = OnceLock::new();

impl ObservabilityAgent {
    pub fn install_global() -> Result<Arc<Self>, ObservabilityError> {
        if let Some(existing) = GLOBAL_AGENT.get() {
            return Ok(Arc::clone(existing));
        }

        register_metric_descriptions();
        let handle = PrometheusBuilder::new()
            .set_buckets_for_metric(
                Matcher::Full("request_duration_ms".to_string()),
                REQUEST_DURATION_BUCKETS,
            )?
            .install_recorder()?;

        let agent = Arc::new(Self {
            handle,
            state: Arc::new(RwLock::new(MutableState::default())),
        });

        let _ = GLOBAL_AGENT.set(Arc::clone(&agent));
        Ok(GLOBAL_AGENT.get().cloned().unwrap_or(agent))
    }

    pub fn global() -> Option<Arc<Self>> {
        GLOBAL_AGENT.get().cloned()
    }

    pub fn router(self: Arc<Self>) -> Router {
        Router::new()
            .route("/metrics", get(metrics_handler))
            .route("/metrics/stream", get(metrics_stream_handler))
            .route("/metrics/snapshot", get(metrics_snapshot_handler))
            .with_state(self)
    }

    pub async fn serve(self: Arc<Self>, listen_addr: &str) -> Result<(), ObservabilityError> {
        let listener = TcpListener::bind(listen_addr).await?;
        axum::serve(listener, self.router()).await?;
        Ok(())
    }

    pub fn metrics_text(&self) -> String {
        self.handle.render()
    }

    pub fn snapshot(&self) -> MetricsSnapshot {
        let mut guard = self.state.write().expect("observability state poisoned");
        prune_state(&mut guard);

        let now_ms = now_ms();
        let request_samples = guard.request_times.len() as f64;
        let error_samples = guard.error_times.len() as f64;
        let req_per_sec = request_samples / SNAPSHOT_WINDOW.as_secs_f64();
        let error_rate = if request_samples == 0.0 {
            0.0
        } else {
            error_samples / request_samples
        };
        let request_duration_p99_ms = percentile_99(&guard.latency_samples);
        let active_connections = guard.active_connections.values().sum();
        let queue_depth = guard.queue_depths.values().sum();
        let cache_hit_rate = average_rate(guard.cache_hit_rates.values().copied());

        let mut component_health = guard.component_health.values().cloned().collect::<Vec<_>>();
        component_health.sort_by(|left, right| left.component.cmp(&right.component));

        MetricsSnapshot {
            timestamp_ms: now_ms,
            request_count: guard.request_count,
            req_per_sec,
            request_duration_p99_ms,
            error_count: guard.error_count,
            error_rate,
            active_connections,
            queue_depth,
            cache_hit_rate,
            component_health,
        }
    }

    pub fn stream_event(&self) -> Result<Event, ObservabilityError> {
        let payload = serde_json::to_string(&self.snapshot())?;
        Ok(Event::default().event("snapshot").data(payload))
    }

    pub fn record_request(&self, component: &str, duration_ms: f64, is_error: bool) {
        counter!("request_count", "component" => component.to_string()).increment(1);
        histogram!("request_duration_ms", "component" => component.to_string()).record(duration_ms);

        let now = Instant::now();
        let mut guard = self.state.write().expect("observability state poisoned");
        guard.request_count = guard.request_count.saturating_add(1);
        guard.request_times.push_back(now);
        guard.latency_samples.push_back((now, duration_ms));

        if is_error {
            counter!("error_count", "component" => component.to_string()).increment(1);
            guard.error_count = guard.error_count.saturating_add(1);
            guard.error_times.push_back(now);
        }

        prune_state(&mut guard);
    }

    pub fn set_active_connections(&self, component: &str, connections: i64) {
        gauge!("active_connections", "component" => component.to_string()).set(connections as f64);
        let mut guard = self.state.write().expect("observability state poisoned");
        guard
            .active_connections
            .insert(component.to_string(), connections.max(0));
    }

    pub fn set_queue_depth(&self, queue: &str, depth: i64) {
        gauge!("queue_depth", "queue" => queue.to_string()).set(depth as f64);
        let mut guard = self.state.write().expect("observability state poisoned");
        guard.queue_depths.insert(queue.to_string(), depth.max(0));
    }

    pub fn set_cache_hit_rate(&self, component: &str, hit_rate: f64) {
        let bounded = hit_rate.clamp(0.0, 1.0);
        gauge!("cache_hit_rate", "component" => component.to_string()).set(bounded);
        let mut guard = self.state.write().expect("observability state poisoned");
        guard.cache_hit_rates.insert(component.to_string(), bounded);
    }

    pub fn update_component_health(
        &self,
        component: &str,
        state: ComponentState,
        detail: impl Into<String>,
    ) {
        let now_ms = now_ms();
        let mut guard = self.state.write().expect("observability state poisoned");
        guard.component_health.insert(
            component.to_string(),
            ComponentHealth {
                component: component.to_string(),
                state,
                detail: detail.into(),
                updated_at_ms: now_ms,
            },
        );
    }

    #[cfg(test)]
    fn reset(&self) {
        let mut guard = self.state.write().expect("observability state poisoned");
        *guard = MutableState::default();
    }
}

fn register_metric_descriptions() {
    describe_counter!(
        "request_count",
        "Total requests observed by Ferrum components."
    );
    describe_histogram!(
        "request_duration_ms",
        "Observed request latency in milliseconds."
    );
    describe_counter!("error_count", "Total failed requests observed.");
    describe_gauge!(
        "active_connections",
        "Current number of open active connections."
    );
    describe_gauge!("queue_depth", "Current total queued jobs.");
    describe_gauge!(
        "cache_hit_rate",
        "Current cache hit rate reported by a Ferrum component."
    );
}

fn prune_state(state: &mut MutableState) {
    let cutoff = Instant::now() - SNAPSHOT_WINDOW;

    while state
        .request_times
        .front()
        .is_some_and(|time| *time < cutoff)
    {
        state.request_times.pop_front();
    }
    while state.error_times.front().is_some_and(|time| *time < cutoff) {
        state.error_times.pop_front();
    }
    while state
        .latency_samples
        .front()
        .is_some_and(|(time, _)| *time < cutoff)
    {
        state.latency_samples.pop_front();
    }
}

fn percentile_99(samples: &VecDeque<(Instant, f64)>) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }

    let mut values = samples.iter().map(|(_, value)| *value).collect::<Vec<_>>();
    values.sort_by(f64::total_cmp);
    let index = ((values.len() as f64 * 0.99).ceil() as usize).saturating_sub(1);
    values[index.min(values.len() - 1)]
}

fn average_rate(values: impl Iterator<Item = f64>) -> f64 {
    let values = values.collect::<Vec<_>>();
    if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f64>() / values.len() as f64
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

async fn metrics_handler(State(agent): State<Arc<ObservabilityAgent>>) -> Response {
    let mut response = agent.metrics_text().into_response();
    response.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; version=0.0.4; charset=utf-8"),
    );
    response
}

async fn metrics_snapshot_handler(
    State(agent): State<Arc<ObservabilityAgent>>,
) -> Json<MetricsSnapshot> {
    Json(agent.snapshot())
}

async fn metrics_stream_handler(
    State(agent): State<Arc<ObservabilityAgent>>,
) -> Sse<impl futures_util::Stream<Item = Result<Event, std::convert::Infallible>>> {
    let stream = stream! {
        let mut interval = tokio::time::interval(STREAM_INTERVAL);
        loop {
            interval.tick().await;
            match agent.stream_event() {
                Ok(event) => yield Ok(event),
                Err(err) => {
                    let fallback = Event::default()
                        .event("snapshot_error")
                        .data(err.to_string());
                    yield Ok(fallback);
                }
            }
        }
    };

    Sse::new(stream).keep_alive(KeepAlive::default())
}

pub fn observe_request(component: &str, duration_ms: f64, is_error: bool) {
    if let Some(agent) = ObservabilityAgent::global() {
        agent.record_request(component, duration_ms, is_error);
    }
}

pub fn observe_active_connections(component: &str, connections: i64) {
    if let Some(agent) = ObservabilityAgent::global() {
        agent.set_active_connections(component, connections);
    }
}

pub fn observe_queue_depth(queue: &str, depth: i64) {
    if let Some(agent) = ObservabilityAgent::global() {
        agent.set_queue_depth(queue, depth);
    }
}

pub fn observe_cache_hit_rate(component: &str, hit_rate: f64) {
    if let Some(agent) = ObservabilityAgent::global() {
        agent.set_cache_hit_rate(component, hit_rate);
    }
}

pub fn observe_component_health(component: &str, state: ComponentState, detail: impl Into<String>) {
    if let Some(agent) = ObservabilityAgent::global() {
        agent.update_component_health(component, state, detail);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;
    use std::sync::OnceLock;
    use tower::util::ServiceExt;

    static TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    fn test_lock() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn test_agent() -> Arc<ObservabilityAgent> {
        let agent = ObservabilityAgent::install_global().expect("global install should work");
        agent.reset();
        agent
    }

    #[test]
    fn snapshot_aggregates_core_metrics() {
        let _guard = test_lock();
        let agent = test_agent();
        agent.record_request("http-server", 12.0, false);
        agent.record_request("gateway", 250.0, true);
        agent.set_active_connections("http-server", 8);
        agent.set_active_connections("gateway", 3);
        agent.set_queue_depth("emails", 42);
        agent.set_cache_hit_rate("proxy", 0.85);
        agent.update_component_health("proxy", ComponentState::Healthy, "ready");

        let snapshot = agent.snapshot();
        assert_eq!(snapshot.request_count, 2);
        assert_eq!(snapshot.error_count, 1);
        assert_eq!(snapshot.active_connections, 11);
        assert_eq!(snapshot.queue_depth, 42);
        assert!((snapshot.cache_hit_rate - 0.85).abs() < f64::EPSILON);
        assert_eq!(snapshot.component_health.len(), 1);
        assert_eq!(snapshot.request_duration_p99_ms, 250.0);
    }

    #[test]
    fn metrics_endpoint_renders_prometheus_text() {
        let _guard = test_lock();
        let agent = test_agent();
        agent.record_request("proxy", 18.0, false);
        agent.set_queue_depth("jobs", 7);

        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let response = runtime.block_on(async {
            agent
                .router()
                .oneshot(
                    axum::http::Request::builder()
                        .uri("/metrics")
                        .body(axum::body::Body::empty())
                        .expect("request"),
                )
                .await
                .expect("response")
        });

        assert_eq!(response.status(), StatusCode::OK);
        let content_type = response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert!(content_type.contains("text/plain"));
    }

    #[test]
    fn stream_event_contains_snapshot_json() {
        let _guard = test_lock();
        let agent = test_agent();
        agent.record_request("queue", 88.0, false);
        let event = agent.stream_event();
        assert!(event.is_ok());
        assert_eq!(agent.snapshot().request_count, 1);
    }
}
