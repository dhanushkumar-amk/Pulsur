use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const DEFAULT_FAILURE_RATE_THRESHOLD: f64 = 50.0;
const DEFAULT_MINIMUM_REQUESTS: usize = 20;
const DEFAULT_ROLLING_WINDOW_SIZE: usize = 100;
const DEFAULT_RESET_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CircuitState {
    Closed,
    Open,
    HalfOpen,
}

#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    pub failure_rate_threshold: f64,
    pub minimum_requests: usize,
    pub rolling_window_size: usize,
    pub reset_timeout: Duration,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_rate_threshold: DEFAULT_FAILURE_RATE_THRESHOLD,
            minimum_requests: DEFAULT_MINIMUM_REQUESTS,
            rolling_window_size: DEFAULT_ROLLING_WINDOW_SIZE,
            reset_timeout: DEFAULT_RESET_TIMEOUT,
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CircuitBreakerError {
    #[error("circuit is open")]
    Open,
    #[error("half-open probe is already in flight")]
    HalfOpenProbeInFlight,
    #[error("downstream request timed out")]
    Timeout,
    #[error("{0}")]
    Downstream(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequestOutcome {
    Success,
    Failure,
    Timeout,
}

#[derive(Debug, Clone)]
struct RollingWindowMetrics {
    buckets: Vec<Option<RequestOutcome>>,
    next_index: usize,
    len: usize,
    success_count: usize,
    failure_count: usize,
    timeout_count: usize,
}

impl RollingWindowMetrics {
    fn new(capacity: usize) -> Self {
        Self {
            buckets: vec![None; capacity.max(1)],
            next_index: 0,
            len: 0,
            success_count: 0,
            failure_count: 0,
            timeout_count: 0,
        }
    }

    fn record(&mut self, outcome: RequestOutcome) {
        if let Some(previous) = self.buckets[self.next_index] {
            self.decrement(previous);
        } else {
            self.len += 1;
        }

        self.buckets[self.next_index] = Some(outcome);
        self.increment(outcome);
        self.next_index = (self.next_index + 1) % self.buckets.len();
    }

    fn clear(&mut self) {
        self.buckets.fill(None);
        self.next_index = 0;
        self.len = 0;
        self.success_count = 0;
        self.failure_count = 0;
        self.timeout_count = 0;
    }

    fn snapshot(&self) -> CircuitMetricsSnapshot {
        let total = self.total_requests();
        let error_total = self.failure_count + self.timeout_count;
        let failure_rate = if total == 0 {
            0.0
        } else {
            (error_total as f64 / total as f64) * 100.0
        };

        CircuitMetricsSnapshot {
            total_requests: total,
            success_count: self.success_count,
            failure_count: self.failure_count,
            timeout_count: self.timeout_count,
            failure_rate,
        }
    }

    fn total_requests(&self) -> usize {
        self.len
    }

    fn increment(&mut self, outcome: RequestOutcome) {
        match outcome {
            RequestOutcome::Success => self.success_count += 1,
            RequestOutcome::Failure => self.failure_count += 1,
            RequestOutcome::Timeout => self.timeout_count += 1,
        }
    }

    fn decrement(&mut self, outcome: RequestOutcome) {
        match outcome {
            RequestOutcome::Success => self.success_count = self.success_count.saturating_sub(1),
            RequestOutcome::Failure => self.failure_count = self.failure_count.saturating_sub(1),
            RequestOutcome::Timeout => self.timeout_count = self.timeout_count.saturating_sub(1),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CircuitMetricsSnapshot {
    pub total_requests: usize,
    pub success_count: usize,
    pub failure_count: usize,
    pub timeout_count: usize,
    pub failure_rate: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CircuitStatus {
    pub name: String,
    pub state: CircuitState,
    pub failure_rate: f64,
    pub total_requests: usize,
    pub success_count: usize,
    pub failure_count: usize,
    pub timeout_count: usize,
    pub minimum_requests: usize,
    pub failure_rate_threshold: f64,
    pub rolling_window_size: usize,
    pub reset_timeout_secs: u64,
}

#[derive(Debug)]
struct CircuitRuntimeState {
    state: CircuitState,
    opened_at: Option<Instant>,
    half_open_probe_in_flight: bool,
}

impl Default for CircuitRuntimeState {
    fn default() -> Self {
        Self {
            state: CircuitState::Closed,
            opened_at: None,
            half_open_probe_in_flight: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExecutionPermit {
    Closed,
    HalfOpenProbe,
}

#[derive(Debug)]
pub struct CircuitBreaker {
    name: String,
    config: CircuitBreakerConfig,
    state: Arc<Mutex<CircuitRuntimeState>>,
    metrics: Arc<Mutex<RollingWindowMetrics>>,
}

impl CircuitBreaker {
    pub fn new(name: impl Into<String>) -> Self {
        Self::with_config(name, CircuitBreakerConfig::default())
    }

    pub fn with_config(name: impl Into<String>, config: CircuitBreakerConfig) -> Self {
        Self {
            name: name.into(),
            metrics: Arc::new(Mutex::new(RollingWindowMetrics::new(
                config.rolling_window_size.max(1),
            ))),
            state: Arc::new(Mutex::new(CircuitRuntimeState::default())),
            config,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn state(&self) -> CircuitState {
        self.state.lock().expect("circuit state poisoned").state
    }

    pub fn metrics(&self) -> CircuitMetricsSnapshot {
        self.metrics.lock().expect("circuit metrics poisoned").snapshot()
    }

    pub fn status(&self) -> CircuitStatus {
        let metrics = self.metrics();
        CircuitStatus {
            name: self.name.clone(),
            state: self.state(),
            failure_rate: metrics.failure_rate,
            total_requests: metrics.total_requests,
            success_count: metrics.success_count,
            failure_count: metrics.failure_count,
            timeout_count: metrics.timeout_count,
            minimum_requests: self.config.minimum_requests,
            failure_rate_threshold: self.config.failure_rate_threshold,
            rolling_window_size: self.config.rolling_window_size,
            reset_timeout_secs: self.config.reset_timeout.as_secs(),
        }
    }

    pub async fn call<F, Fut, T>(&self, operation: F) -> Result<T, CircuitBreakerError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T, CircuitBreakerError>>,
    {
        let permit = self.try_acquire_permission_at(Instant::now())?;
        let result = operation().await;
        self.record_result_at(Instant::now(), permit, result.as_ref().map(|_| ()).map_err(Clone::clone));
        result
    }

    pub async fn call_with_timeout<F, Fut, T>(
        &self,
        timeout: Duration,
        operation: F,
    ) -> Result<T, CircuitBreakerError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T, CircuitBreakerError>>,
    {
        let permit = self.try_acquire_permission_at(Instant::now())?;
        let result = match tokio::time::timeout(timeout, operation()).await {
            Ok(result) => result,
            Err(_) => Err(CircuitBreakerError::Timeout),
        };

        self.record_result_at(Instant::now(), permit, result.as_ref().map(|_| ()).map_err(Clone::clone));
        result
    }

    fn try_acquire_permission_at(
        &self,
        now: Instant,
    ) -> Result<ExecutionPermit, CircuitBreakerError> {
        let mut guard = self.state.lock().expect("circuit state poisoned");

        if guard.state == CircuitState::Open
            && guard
                .opened_at
                .is_some_and(|opened_at| now.duration_since(opened_at) >= self.config.reset_timeout)
        {
            guard.state = CircuitState::HalfOpen;
            guard.half_open_probe_in_flight = false;
            guard.opened_at = None;
        }

        match guard.state {
            CircuitState::Closed => Ok(ExecutionPermit::Closed),
            CircuitState::Open => Err(CircuitBreakerError::Open),
            CircuitState::HalfOpen => {
                if guard.half_open_probe_in_flight {
                    Err(CircuitBreakerError::HalfOpenProbeInFlight)
                } else {
                    guard.half_open_probe_in_flight = true;
                    Ok(ExecutionPermit::HalfOpenProbe)
                }
            }
        }
    }

    fn record_result_at(
        &self,
        now: Instant,
        permit: ExecutionPermit,
        result: Result<(), CircuitBreakerError>,
    ) {
        let outcome = match result {
            Ok(()) => RequestOutcome::Success,
            Err(CircuitBreakerError::Timeout) => RequestOutcome::Timeout,
            Err(CircuitBreakerError::Open) | Err(CircuitBreakerError::HalfOpenProbeInFlight) => {
                return;
            }
            Err(CircuitBreakerError::Downstream(_)) => RequestOutcome::Failure,
        };

        let metrics = {
            let mut metrics = self.metrics.lock().expect("circuit metrics poisoned");
            metrics.record(outcome);
            metrics.snapshot()
        };

        let mut state = self.state.lock().expect("circuit state poisoned");
        match permit {
            ExecutionPermit::Closed => {
                if metrics.total_requests >= self.config.minimum_requests
                    && metrics.failure_rate > self.config.failure_rate_threshold
                {
                    state.state = CircuitState::Open;
                    state.opened_at = Some(now);
                    state.half_open_probe_in_flight = false;
                }
            }
            ExecutionPermit::HalfOpenProbe => match outcome {
                RequestOutcome::Success => {
                    state.state = CircuitState::Closed;
                    state.opened_at = None;
                    state.half_open_probe_in_flight = false;
                    self.metrics.lock().expect("circuit metrics poisoned").clear();
                }
                RequestOutcome::Failure | RequestOutcome::Timeout => {
                    state.state = CircuitState::Open;
                    state.opened_at = Some(now);
                    state.half_open_probe_in_flight = false;
                }
            },
        }
    }
}

#[derive(Debug, Default)]
pub struct CircuitBreakerRegistry {
    circuits: Arc<RwLock<HashMap<String, Arc<CircuitBreaker>>>>,
}

impl CircuitBreakerRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&self, breaker: Arc<CircuitBreaker>) {
        let mut guard = self.circuits.write().expect("circuit registry poisoned");
        guard.insert(breaker.name().to_string(), breaker);
    }

    pub fn get(&self, name: &str) -> Option<Arc<CircuitBreaker>> {
        self.circuits
            .read()
            .expect("circuit registry poisoned")
            .get(name)
            .cloned()
    }

    pub fn router(self: Arc<Self>) -> Router {
        Router::new()
            .route("/circuit/:name/status", get(get_circuit_status))
            .with_state(self)
    }
}

async fn get_circuit_status(
    State(registry): State<Arc<CircuitBreakerRegistry>>,
    Path(name): Path<String>,
) -> Result<Json<CircuitStatus>, StatusCode> {
    registry
        .get(&name)
        .map(|breaker| Json(breaker.status()))
        .ok_or(StatusCode::NOT_FOUND)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;
    use tower::util::ServiceExt;

    fn test_config() -> CircuitBreakerConfig {
        CircuitBreakerConfig {
            failure_rate_threshold: 50.0,
            minimum_requests: 4,
            rolling_window_size: 8,
            reset_timeout: Duration::from_secs(30),
        }
    }

    #[test]
    fn rolling_window_keeps_only_the_last_n_requests() {
        let mut metrics = RollingWindowMetrics::new(4);
        metrics.record(RequestOutcome::Success);
        metrics.record(RequestOutcome::Failure);
        metrics.record(RequestOutcome::Timeout);
        metrics.record(RequestOutcome::Success);
        metrics.record(RequestOutcome::Failure);

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.total_requests, 4);
        assert_eq!(snapshot.success_count, 1);
        assert_eq!(snapshot.failure_count, 2);
        assert_eq!(snapshot.timeout_count, 1);
        assert!((snapshot.failure_rate - 75.0).abs() < f64::EPSILON);
    }

    #[test]
    fn circuit_opens_when_failure_rate_threshold_is_exceeded() {
        let breaker = CircuitBreaker::with_config("payments", test_config());
        let now = Instant::now();

        for _ in 0..3 {
            let permit = breaker.try_acquire_permission_at(now).expect("closed circuit should allow");
            breaker.record_result_at(
                now,
                permit,
                Err(CircuitBreakerError::Downstream("upstream failed".to_string())),
            );
        }

        assert_eq!(breaker.state(), CircuitState::Closed);

        let permit = breaker.try_acquire_permission_at(now).expect("closed circuit should allow");
        breaker.record_result_at(
            now,
            permit,
            Ok(()),
        );

        assert_eq!(breaker.state(), CircuitState::Open);
        assert_eq!(breaker.metrics().failure_count, 3);
        assert_eq!(breaker.metrics().success_count, 1);
    }

    #[test]
    fn open_circuit_transitions_to_half_open_after_reset_timeout() {
        let breaker = CircuitBreaker::with_config("payments", test_config());
        let now = Instant::now();

        for _ in 0..4 {
            let permit = breaker.try_acquire_permission_at(now).expect("closed circuit should allow");
            breaker.record_result_at(
                now,
                permit,
                Err(CircuitBreakerError::Downstream("boom".to_string())),
            );
        }

        assert_eq!(breaker.state(), CircuitState::Open);
        assert!(matches!(
            breaker.try_acquire_permission_at(now + Duration::from_secs(29)),
            Err(CircuitBreakerError::Open)
        ));

        let permit = breaker
            .try_acquire_permission_at(now + Duration::from_secs(30))
            .expect("half-open probe should be allowed");
        assert_eq!(permit, ExecutionPermit::HalfOpenProbe);
        assert_eq!(breaker.state(), CircuitState::HalfOpen);
        assert!(matches!(
            breaker.try_acquire_permission_at(now + Duration::from_secs(30)),
            Err(CircuitBreakerError::HalfOpenProbeInFlight)
        ));
    }

    #[test]
    fn half_open_success_closes_and_resets_metrics() {
        let breaker = CircuitBreaker::with_config("payments", test_config());
        let now = Instant::now();

        for _ in 0..4 {
            let permit = breaker.try_acquire_permission_at(now).expect("closed circuit should allow");
            breaker.record_result_at(
                now,
                permit,
                Err(CircuitBreakerError::Downstream("boom".to_string())),
            );
        }

        let permit = breaker
            .try_acquire_permission_at(now + Duration::from_secs(31))
            .expect("half-open probe should be allowed");
        breaker.record_result_at(now + Duration::from_secs(31), permit, Ok(()));

        assert_eq!(breaker.state(), CircuitState::Closed);
        let metrics = breaker.metrics();
        assert_eq!(metrics.total_requests, 0);
        assert_eq!(metrics.failure_count, 0);
    }

    #[test]
    fn half_open_failure_reopens_circuit() {
        let breaker = CircuitBreaker::with_config("payments", test_config());
        let now = Instant::now();

        for _ in 0..4 {
            let permit = breaker.try_acquire_permission_at(now).expect("closed circuit should allow");
            breaker.record_result_at(
                now,
                permit,
                Err(CircuitBreakerError::Downstream("boom".to_string())),
            );
        }

        let permit = breaker
            .try_acquire_permission_at(now + Duration::from_secs(31))
            .expect("half-open probe should be allowed");
        breaker.record_result_at(
            now + Duration::from_secs(31),
            permit,
            Err(CircuitBreakerError::Timeout),
        );

        assert_eq!(breaker.state(), CircuitState::Open);
    }

    #[tokio::test]
    async fn call_with_timeout_records_timeout_and_opens_circuit() {
        let breaker = CircuitBreaker::with_config(
            "search",
            CircuitBreakerConfig {
                failure_rate_threshold: 50.0,
                minimum_requests: 2,
                rolling_window_size: 4,
                reset_timeout: Duration::from_secs(1),
            },
        );

        let first = breaker
            .call_with_timeout(Duration::from_millis(10), || async {
                std::future::pending::<()>().await;
                Ok::<_, CircuitBreakerError>("ok")
            })
            .await;
        assert_eq!(first, Err(CircuitBreakerError::Timeout));

        let second = breaker
            .call(|| async {
                Err::<(), _>(CircuitBreakerError::Downstream("upstream failed".to_string()))
            })
            .await;
        assert!(matches!(second, Err(CircuitBreakerError::Downstream(_))));
        assert_eq!(breaker.state(), CircuitState::Open);
        assert_eq!(breaker.metrics().timeout_count, 1);
    }

    #[tokio::test]
    async fn status_router_returns_circuit_metrics() {
        let breaker = Arc::new(CircuitBreaker::with_config("billing", test_config()));
        let registry = Arc::new(CircuitBreakerRegistry::new());
        registry.register(breaker.clone());

        let permit = breaker
            .try_acquire_permission_at(Instant::now())
            .expect("closed circuit should allow");
        breaker.record_result_at(
            Instant::now(),
            permit,
            Err(CircuitBreakerError::Downstream("failed".to_string())),
        );

        let app = registry.router();
        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/circuit/billing/status")
                    .body(axum::body::Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("router should respond");

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should read");
        let status: CircuitStatus =
            serde_json::from_slice(&body).expect("status payload should deserialize");
        assert_eq!(status.name, "billing");
        assert_eq!(status.failure_count, 1);
        assert_eq!(status.state, CircuitState::Closed);
    }
}
