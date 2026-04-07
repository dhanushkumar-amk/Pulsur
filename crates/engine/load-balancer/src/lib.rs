use std::hash::Hasher;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tokio::task::JoinSet;
use tracing::{debug, info, warn};

const DEFAULT_HEALTH_CHECK_INTERVAL: Duration = Duration::from_secs(5);
const DEFAULT_HEALTH_CHECK_TIMEOUT: Duration = Duration::from_secs(2);
const DEFAULT_SESSION_TTL: Duration = Duration::from_secs(30 * 60);
const DEFAULT_SESSION_CLEANUP_INTERVAL: Duration = Duration::from_secs(60);
const DEFAULT_DRAIN_TIMEOUT: Duration = Duration::from_secs(60);
const DEFAULT_DRAIN_LOG_INTERVAL: Duration = Duration::from_secs(10);
const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;

/// Represents a single backend server in the load balancer pool.
#[derive(Debug)]
pub struct Backend {
    pub address: String,
    pub weight: u32,
    pub healthy: AtomicBool,
    pub draining: AtomicBool,
    pub active_conns: AtomicUsize,
}

impl Backend {
    pub fn new(address: &str, weight: u32) -> Self {
        Self {
            address: address.to_string(),
            weight,
            healthy: AtomicBool::new(true),
            draining: AtomicBool::new(false),
            active_conns: AtomicUsize::new(0),
        }
    }

    pub fn health_check_url(&self) -> String {
        format!("http://{}/health", self.address)
    }

    pub fn set_healthy(&self, status: bool) {
        self.healthy.store(status, Ordering::Relaxed);
    }

    pub fn is_healthy(&self) -> bool {
        self.healthy.load(Ordering::Relaxed)
    }

    pub fn set_draining(&self, status: bool) {
        self.draining.store(status, Ordering::Relaxed);
    }

    pub fn is_draining(&self) -> bool {
        self.draining.load(Ordering::Relaxed)
    }

    pub fn is_routable(&self) -> bool {
        self.is_healthy() && !self.is_draining()
    }
}

/// Runtime configuration for the load balancer pool.
#[derive(Debug, Clone)]
pub struct BackendPoolConfig {
    pub sticky_sessions: bool,
    pub health_check_interval: Duration,
    pub health_check_timeout: Duration,
    pub session_ttl: Duration,
    pub session_cleanup_interval: Duration,
    pub drain_timeout: Duration,
    pub drain_log_interval: Duration,
}

impl Default for BackendPoolConfig {
    fn default() -> Self {
        Self {
            sticky_sessions: false,
            health_check_interval: DEFAULT_HEALTH_CHECK_INTERVAL,
            health_check_timeout: DEFAULT_HEALTH_CHECK_TIMEOUT,
            session_ttl: DEFAULT_SESSION_TTL,
            session_cleanup_interval: DEFAULT_SESSION_CLEANUP_INTERVAL,
            drain_timeout: DEFAULT_DRAIN_TIMEOUT,
            drain_log_interval: DEFAULT_DRAIN_LOG_INTERVAL,
        }
    }
}

#[derive(Debug, Clone)]
struct SessionEntry {
    backend_index: usize,
    expires_at: Instant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTarget {
    pub key: String,
    pub backend_index: usize,
}

/// A summary of the backend status for the monitoring endpoint.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackendStatus {
    pub address: String,
    pub weight: u32,
    pub healthy: bool,
    pub draining: bool,
    pub active_conns: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DrainResponse {
    pub address: String,
    pub draining: bool,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrainBackendError {
    NotFound,
    AlreadyDraining,
}

/// A pool of backends with various load balancing algorithms.
pub struct BackendPool {
    backends: Arc<RwLock<Vec<Arc<Backend>>>>,
    rr_counter: AtomicUsize,
    config: BackendPoolConfig,
    sessions: DashMap<String, SessionEntry>,
    health_checker_started: AtomicBool,
    session_cleanup_started: AtomicBool,
}

impl BackendPool {
    /// Creates a new empty backend pool.
    pub fn new() -> Self {
        Self::from_config(BackendPoolConfig::default())
    }

    /// Creates a pool from an explicit config.
    pub fn from_config(config: BackendPoolConfig) -> Self {
        Self {
            backends: Arc::new(RwLock::new(Vec::new())),
            rr_counter: AtomicUsize::new(0),
            config,
            sessions: DashMap::new(),
            health_checker_started: AtomicBool::new(false),
            session_cleanup_started: AtomicBool::new(false),
        }
    }

    /// Enables or disables sticky sessions.
    pub fn with_sticky_sessions(mut self, sticky_sessions: bool) -> Self {
        self.config.sticky_sessions = sticky_sessions;
        self
    }

    /// Sets the health check interval.
    pub fn with_health_check_interval(mut self, interval: Duration) -> Self {
        self.config.health_check_interval = interval;
        self
    }

    /// Sets the health check timeout.
    pub fn with_health_check_timeout(mut self, timeout: Duration) -> Self {
        self.config.health_check_timeout = timeout;
        self
    }

    /// Sets the sticky-session TTL.
    pub fn with_session_ttl(mut self, ttl: Duration) -> Self {
        self.config.session_ttl = ttl;
        self
    }

    /// Sets the sticky-session cleanup interval.
    pub fn with_session_cleanup_interval(mut self, interval: Duration) -> Self {
        self.config.session_cleanup_interval = interval;
        self
    }

    /// Sets the maximum duration a backend is allowed to stay in drain mode.
    pub fn with_drain_timeout(mut self, timeout: Duration) -> Self {
        self.config.drain_timeout = timeout;
        self
    }

    /// Sets how often drain progress is logged.
    pub fn with_drain_log_interval(mut self, interval: Duration) -> Self {
        self.config.drain_log_interval = interval;
        self
    }

    /// Adds a backend to the pool.
    pub fn add(&self, backend: Backend) {
        let mut backends = self.backends.write().unwrap_or_else(|e| e.into_inner());
        backends.push(Arc::new(backend));
    }

    /// Removes a backend from the pool by address.
    pub fn remove(&self, address: &str) {
        let mut backends = self.backends.write().unwrap_or_else(|e| e.into_inner());
        backends.retain(|backend| backend.address != address);
        self.sessions
            .retain(|_, entry| entry.backend_index < backends.len());
    }

    /// Puts a backend into drain mode and removes it after existing traffic clears or times out.
    pub fn drain_backend(self: &Arc<Self>, address: &str) -> Result<(), DrainBackendError> {
        let backend = self
            .backend_by_address(address)
            .ok_or(DrainBackendError::NotFound)?;

        if backend
            .draining
            .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
        {
            return Err(DrainBackendError::AlreadyDraining);
        }

        let address = backend.address.clone();
        let pool = Arc::clone(self);
        let backend_for_task = Arc::clone(&backend);

        info!(
            "starting drain for backend {} with {} active connections",
            address,
            backend_for_task.active_conns.load(Ordering::Relaxed)
        );

        // Use tokio::spawn so the drain loop runs on the async runtime;
        // tokio::time::sleep avoids blocking a thread-pool thread.
        tokio::spawn(async move {
            let started_at = tokio::time::Instant::now();

            loop {
                let active_conns = backend_for_task.active_conns.load(Ordering::Relaxed);
                if active_conns == 0 {
                    pool.remove(&address);
                    info!(
                        "drain complete for backend {} with final active connections {}",
                        address, active_conns
                    );
                    break;
                }

                if started_at.elapsed() >= pool.config.drain_timeout {
                    pool.remove(&address);
                    warn!(
                        "drain timeout reached for backend {}; force removed with final active connections {}",
                        address, active_conns
                    );
                    break;
                }

                info!(
                    "drain in progress for backend {}: {} active connections remaining",
                    address, active_conns
                );

                // Non-blocking sleep: yields to the tokio scheduler.
                tokio::time::sleep(pool.config.drain_log_interval).await;
            }
        });

        Ok(())
    }

    /// Selects the next healthy backend using Round Robin.
    pub fn next_round_robin(&self) -> Option<Arc<Backend>> {
        let healthy_backends = self.healthy_backends();
        if healthy_backends.is_empty() {
            return None;
        }

        let index = self.rr_counter.fetch_add(1, Ordering::Relaxed) % healthy_backends.len();
        Some(Arc::clone(&healthy_backends[index]))
    }

    /// Selects the healthy backend with the lowest active connection count.
    pub fn next_least_connections(&self) -> Option<Arc<Backend>> {
        self.healthy_backends()
            .into_iter()
            .min_by_key(|backend| backend.active_conns.load(Ordering::Relaxed))
    }

    /// Selects the next healthy backend using an expanded weighted round-robin pool.
    pub fn next_weighted_round_robin(&self) -> Option<Arc<Backend>> {
        let expanded_pool = self.expanded_weighted_backends();
        if expanded_pool.is_empty() {
            return None;
        }

        let index = self.rr_counter.fetch_add(1, Ordering::Relaxed) % expanded_pool.len();
        Some(Arc::clone(&expanded_pool[index]))
    }

    /// Selects a sticky backend using `session=<id>` from the cookie header or the client IP.
    pub fn next_sticky(
        &self,
        cookie_header: Option<&str>,
        client_ip: Option<&str>,
    ) -> Option<Arc<Backend>> {
        if !self.config.sticky_sessions {
            return self.next_round_robin();
        }

        let session_key = Self::extract_session_key(cookie_header, client_ip)?;
        let (backend, _) = self.select_sticky_backend(&session_key)?;
        Some(backend)
    }

    /// Selects a sticky backend and returns the resolved session mapping.
    pub fn next_sticky_with_target(
        &self,
        cookie_header: Option<&str>,
        client_ip: Option<&str>,
    ) -> Option<(Arc<Backend>, SessionTarget)> {
        if !self.config.sticky_sessions {
            let backend = self.next_round_robin()?;
            let backend_index = self.backend_index_by_address(&backend.address)?;
            return Some((
                backend,
                SessionTarget {
                    key: client_ip.unwrap_or_default().to_string(),
                    backend_index,
                },
            ));
        }

        let session_key = Self::extract_session_key(cookie_header, client_ip)?;
        let (backend, backend_index) = self.select_sticky_backend(&session_key)?;
        Some((
            backend,
            SessionTarget {
                key: session_key,
                backend_index,
            },
        ))
    }

    /// Spawns one background task that continuously probes all backends.
    pub fn spawn_health_checker(self: Arc<Self>) {
        if self
            .health_checker_started
            .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
        {
            debug!("health checker already running");
            return;
        }

        tokio::spawn(async move {
            let client = reqwest::Client::builder()
                .connect_timeout(self.config.health_check_timeout)
                .timeout(self.config.health_check_timeout)
                .build()
                .expect("failed to build health-check client");

            let mut interval = tokio::time::interval(self.config.health_check_interval);

            loop {
                interval.tick().await;
                let backends = self.backends_snapshot();
                let mut checks = JoinSet::new();

                for backend in backends {
                    let client = client.clone();
                    checks.spawn(async move {
                        let response = client.get(backend.health_check_url()).send().await;
                        let healthy = matches!(&response, Ok(resp) if resp.status().is_success());

                        if healthy {
                            if !backend.is_healthy() {
                                info!("backend {} recovered and was re-added to routing", backend.address);
                            }
                            backend.set_healthy(true);
                        } else {
                            if backend.is_healthy() {
                                match response {
                                    Ok(resp) => {
                                        warn!(
                                            "backend {} marked unhealthy after /health returned {}",
                                            backend.address,
                                            resp.status()
                                        );
                                    }
                                    Err(err) => {
                                        warn!(
                                            "backend {} marked unhealthy after health check failed: {}",
                                            backend.address,
                                            err
                                        );
                                    }
                                }
                            }
                            backend.set_healthy(false);
                        }
                    });
                }

                while checks.join_next().await.is_some() {}
            }
        });
    }

    /// Spawns a cleanup task that evicts expired sticky sessions.
    pub fn spawn_session_cleanup(self: Arc<Self>) {
        if !self.config.sticky_sessions {
            debug!("sticky sessions disabled; cleanup task not started");
            return;
        }

        if self
            .session_cleanup_started
            .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
        {
            debug!("session cleanup already running");
            return;
        }

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(self.config.session_cleanup_interval);

            loop {
                interval.tick().await;
                let now = Instant::now();
                self.sessions.retain(|_, entry| entry.expires_at > now);
            }
        });
    }

    /// Returns a list of all backends with their current status.
    pub fn get_backends_status(&self) -> Vec<BackendStatus> {
        self.backends_snapshot()
            .into_iter()
            .map(|backend| BackendStatus {
                address: backend.address.clone(),
                weight: backend.weight,
                healthy: backend.is_healthy(),
                draining: backend.is_draining(),
                active_conns: backend.active_conns.load(Ordering::Relaxed),
            })
            .collect()
    }

    /// Returns an Axum router for the load balancer management endpoints.
    pub fn status_router(self: Arc<Self>) -> Router {
        Router::new()
            .route("/lb/backends", get(list_backends))
            .route("/lb/backends/:addr/drain", post(drain_backend))
            .with_state(self)
    }

    fn select_sticky_backend(&self, session_key: &str) -> Option<(Arc<Backend>, usize)> {
        let now = Instant::now();

        if let Some(mut entry) = self.sessions.get_mut(session_key) {
            if entry.expires_at > now {
                if let Some(backend) = self.healthy_backend_at_index(entry.backend_index) {
                    entry.expires_at = now + self.config.session_ttl;
                    return Some((backend, entry.backend_index));
                }
            }
        }

        let backends = self.backends_snapshot();
        if backends.is_empty() {
            self.sessions.remove(session_key);
            return None;
        }

        let hashed_index = fnv1a_hash(session_key.as_bytes()) as usize % backends.len();
        if let Some(backend) = backends
            .get(hashed_index)
            .cloned()
            .filter(|backend| backend.is_healthy())
        {
            self.upsert_session(session_key, hashed_index, now);
            return Some((backend, hashed_index));
        }

        let backend = self.next_round_robin()?;
        let backend_index = self.backend_index_by_address(&backend.address)?;
        self.upsert_session(session_key, backend_index, now);
        Some((backend, backend_index))
    }

    fn upsert_session(&self, session_key: &str, backend_index: usize, now: Instant) {
        self.sessions.insert(
            session_key.to_string(),
            SessionEntry {
                backend_index,
                expires_at: now + self.config.session_ttl,
            },
        );
    }

    fn healthy_backend_at_index(&self, backend_index: usize) -> Option<Arc<Backend>> {
        self.backends_snapshot()
            .get(backend_index)
            .cloned()
            .filter(|backend| backend.is_healthy())
    }

    fn backend_index_by_address(&self, address: &str) -> Option<usize> {
        self.backends
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .position(|backend| backend.address == address)
    }

    fn backend_by_address(&self, address: &str) -> Option<Arc<Backend>> {
        self.backends
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .find(|backend| backend.address == address)
            .cloned()
    }

    pub fn backends_snapshot(&self) -> Vec<Arc<Backend>> {
        self.backends
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .cloned()
            .collect()
    }

    fn healthy_backends(&self) -> Vec<Arc<Backend>> {
        self.backends_snapshot()
            .into_iter()
            .filter(|backend| backend.is_routable())
            .collect()
    }

    fn expanded_weighted_backends(&self) -> Vec<Arc<Backend>> {
        let healthy_backends = self.healthy_backends();
        let mut expanded = Vec::new();

        for backend in healthy_backends {
            let copies = backend.weight.max(1) as usize;
            expanded.extend(
                std::iter::repeat_with({
                    let backend = Arc::clone(&backend);
                    move || Arc::clone(&backend)
                })
                .take(copies),
            );
        }

        expanded
    }

    fn extract_session_key(cookie_header: Option<&str>, client_ip: Option<&str>) -> Option<String> {
        if let Some(cookie_header) = cookie_header {
            for cookie in cookie_header.split(';') {
                let trimmed = cookie.trim();
                if let Some(value) = trimmed.strip_prefix("session=") {
                    if !value.is_empty() {
                        return Some(value.to_string());
                    }
                }
            }
        }

        client_ip.map(str::to_string)
    }
}

async fn list_backends(State(pool): State<Arc<BackendPool>>) -> Json<Vec<BackendStatus>> {
    Json(pool.get_backends_status())
}

async fn drain_backend(
    Path(addr): Path<String>,
    State(pool): State<Arc<BackendPool>>,
) -> impl IntoResponse {
    match pool.drain_backend(&addr) {
        Ok(()) => (
            StatusCode::ACCEPTED,
            Json(DrainResponse {
                address: addr,
                draining: true,
                message: "backend drain started".to_string(),
            }),
        )
            .into_response(),
        Err(DrainBackendError::NotFound) => (
            StatusCode::NOT_FOUND,
            Json(DrainResponse {
                address: addr,
                draining: false,
                message: "backend not found".to_string(),
            }),
        )
            .into_response(),
        Err(DrainBackendError::AlreadyDraining) => (
            StatusCode::CONFLICT,
            Json(DrainResponse {
                address: addr,
                draining: true,
                message: "backend is already draining".to_string(),
            }),
        )
            .into_response(),
    }
}

impl Default for BackendPool {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Default)]
struct Fnv1aHasher(u64);

impl Hasher for Fnv1aHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        let mut hash = FNV_OFFSET_BASIS;
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(FNV_PRIME);
        }
        self.0 = hash;
    }
}

fn fnv1a_hash(bytes: &[u8]) -> u64 {
    let mut hasher = Fnv1aHasher::default();
    hasher.write(bytes);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use axum::body::{to_bytes, Body};
    use http::{Request, StatusCode};
    use tower::ServiceExt;

    use super::*;

    #[test]
    fn test_runtime_add_remove() {
        let pool = BackendPool::new();
        pool.add(Backend::new("127.0.0.1:8080", 1));
        pool.add(Backend::new("127.0.0.1:8081", 1));

        assert_eq!(pool.get_backends_status().len(), 2);

        pool.remove("127.0.0.1:8080");

        let statuses = pool.get_backends_status();
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].address, "127.0.0.1:8081");
    }

    #[test]
    fn test_health_filter() {
        let pool = BackendPool::new();
        let b1 = Backend::new("127.0.0.1:8080", 1);
        let b2 = Backend::new("127.0.0.1:8081", 1);

        b1.set_healthy(false);

        pool.add(b1);
        pool.add(b2);

        for _ in 0..10 {
            let selected = pool.next_round_robin().expect("expected healthy backend");
            assert_eq!(selected.address, "127.0.0.1:8081");
        }
    }

    #[tokio::test]
    async fn test_draining_backend_is_removed_from_routing() {
        let pool = Arc::new(BackendPool::new());
        let draining = Backend::new("b0", 1);
        let healthy = Backend::new("b1", 1);

        pool.add(draining);
        pool.add(healthy);
        pool.drain_backend("b0").expect("drain should start");

        for _ in 0..10 {
            let selected = pool.next_round_robin().expect("expected routable backend");
            assert_eq!(selected.address, "b1");
        }
    }

    #[test]
    fn test_no_healthy_backends() {
        let pool = BackendPool::new();
        let backend = Backend::new("127.0.0.1:8080", 1);
        backend.set_healthy(false);
        pool.add(backend);

        assert!(pool.next_round_robin().is_none());
        assert!(pool.next_least_connections().is_none());
        assert!(pool.next_weighted_round_robin().is_none());
    }

    #[test]
    fn test_distribution_with_weights() {
        let pool = BackendPool::new();
        pool.add(Backend::new("b1", 3));
        pool.add(Backend::new("b2", 1));

        let mut counts = HashMap::new();
        for _ in 0..100 {
            let backend = pool
                .next_weighted_round_robin()
                .expect("expected weighted backend");
            *counts.entry(backend.address.clone()).or_insert(0usize) += 1;
        }

        assert_eq!(counts["b1"], 75);
        assert_eq!(counts["b2"], 25);
    }

    #[test]
    fn test_distribution_round_robin_even() {
        let pool = BackendPool::new();
        for i in 0..5 {
            pool.add(Backend::new(&format!("b{i}"), 1));
        }

        let mut counts = HashMap::new();
        for _ in 0..1000 {
            let backend = pool
                .next_round_robin()
                .expect("expected round-robin backend");
            *counts.entry(backend.address.clone()).or_insert(0usize) += 1;
        }

        assert_eq!(counts.len(), 5);
        for count in counts.values() {
            assert_eq!(*count, 200);
        }
    }

    #[test]
    fn test_least_connections_prefers_lowest_count() {
        let pool = BackendPool::new();

        let b1 = Backend::new("b1", 1);
        let b2 = Backend::new("b2", 1);
        let b3 = Backend::new("b3", 1);

        b1.active_conns.store(4, Ordering::Relaxed);
        b2.active_conns.store(1, Ordering::Relaxed);
        b3.active_conns.store(2, Ordering::Relaxed);

        pool.add(b1);
        pool.add(b2);
        pool.add(b3);

        let backend = pool
            .next_least_connections()
            .expect("expected least-connections backend");
        assert_eq!(backend.address, "b2");
    }

    #[test]
    fn test_extract_session_key_prefers_cookie_then_ip() {
        assert_eq!(
            BackendPool::extract_session_key(
                Some("foo=bar; session=abc123; theme=dark"),
                Some("10.0.0.1")
            ),
            Some("abc123".to_string())
        );
        assert_eq!(
            BackendPool::extract_session_key(Some("foo=bar"), Some("10.0.0.1")),
            Some("10.0.0.1".to_string())
        );
    }

    #[test]
    fn test_sticky_sessions_same_client_ip_hits_same_backend() {
        let pool = BackendPool::new().with_sticky_sessions(true);
        for i in 0..5 {
            pool.add(Backend::new(&format!("b{i}"), 1));
        }

        let mut selected = HashMap::new();
        for _ in 0..1000 {
            let backend = pool
                .next_sticky(None, Some("192.168.1.10"))
                .expect("expected sticky backend");
            *selected.entry(backend.address.clone()).or_insert(0usize) += 1;
        }

        assert_eq!(selected.len(), 1);
        assert_eq!(selected.values().next().copied(), Some(1000));
    }

    #[test]
    fn test_sticky_sessions_fall_back_when_hashed_backend_unhealthy() {
        let pool = BackendPool::new().with_sticky_sessions(true);
        pool.add(Backend::new("b0", 1));
        pool.add(Backend::new("b1", 1));
        pool.add(Backend::new("b2", 1));

        let key = "10.0.0.44";
        let hashed_index = fnv1a_hash(key.as_bytes()) as usize % 3;
        let hashed_backend = pool.backends_snapshot()[hashed_index].clone();
        hashed_backend.set_healthy(false);

        let (selected, target) = pool
            .next_sticky_with_target(None, Some(key))
            .expect("expected fallback backend");

        assert_ne!(target.backend_index, hashed_index);
        assert!(selected.is_healthy());

        let repeated = pool
            .next_sticky(None, Some(key))
            .expect("expected persisted fallback backend");
        assert_eq!(selected.address, repeated.address);
    }

    #[tokio::test]
    async fn test_session_cleanup_expires_entries() {
        let pool = Arc::new(
            BackendPool::new()
                .with_sticky_sessions(true)
                .with_session_ttl(Duration::from_millis(50))
                .with_session_cleanup_interval(Duration::from_millis(20)),
        );
        pool.add(Backend::new("b0", 1));

        let _ = pool.next_sticky(None, Some("172.16.0.1"));
        assert_eq!(pool.sessions.len(), 1);

        pool.clone().spawn_session_cleanup();

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if pool.sessions.is_empty() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("session entry should expire and be cleaned up");
    }

    #[tokio::test]
    async fn test_status_router_lists_backend_health() {
        let pool = Arc::new(BackendPool::new());
        let backend = Backend::new("127.0.0.1:8080", 3);
        backend.set_healthy(false);
        backend.set_draining(true);
        backend.active_conns.store(7, Ordering::Relaxed);
        pool.add(backend);

        let response = pool
            .clone()
            .status_router()
            .oneshot(
                Request::builder()
                    .uri("/lb/backends")
                    .method("GET")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("router should respond");

        assert_eq!(response.status(), StatusCode::OK);

        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should be readable");
        let statuses: Vec<BackendStatus> =
            serde_json::from_slice(&body).expect("response should be valid json");

        assert_eq!(
            statuses,
            vec![BackendStatus {
                address: "127.0.0.1:8080".to_string(),
                weight: 3,
                healthy: false,
                draining: true,
                active_conns: 7,
            }]
        );
    }

    #[tokio::test]
    async fn test_drain_router_marks_backend_draining() {
        let pool = Arc::new(BackendPool::new());
        let backend = Backend::new("b0", 1);
        backend.active_conns.store(2, Ordering::Relaxed);
        pool.add(backend);

        let response = pool
            .clone()
            .status_router()
            .oneshot(
                Request::builder()
                    .uri("/lb/backends/b0/drain")
                    .method("POST")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("router should respond");

        assert_eq!(response.status(), StatusCode::ACCEPTED);

        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should be readable");
        let payload: DrainResponse =
            serde_json::from_slice(&body).expect("response should be valid json");

        assert_eq!(
            payload,
            DrainResponse {
                address: "b0".to_string(),
                draining: true,
                message: "backend drain started".to_string(),
            }
        );
        assert!(pool.get_backends_status()[0].draining);
    }

    #[tokio::test]
    async fn test_drain_removes_backend_once_connections_reach_zero() {
        let pool = Arc::new(
            BackendPool::new()
                .with_drain_timeout(Duration::from_millis(200))
                .with_drain_log_interval(Duration::from_millis(20)),
        );
        let backend = Backend::new("b0", 1);
        backend.active_conns.store(1, Ordering::Relaxed);
        pool.add(backend);

        pool.drain_backend("b0").expect("drain should start");
        assert_eq!(pool.get_backends_status().len(), 1);

        tokio::time::sleep(Duration::from_millis(30)).await;
        let drained_backend = pool
            .backend_by_address("b0")
            .expect("backend should still exist while draining");
        drained_backend.active_conns.store(0, Ordering::Relaxed);

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if pool.get_backends_status().is_empty() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("backend should be removed after active connections drain");
    }

    #[tokio::test]
    async fn test_drain_force_removes_backend_after_timeout() {
        let pool = Arc::new(
            BackendPool::new()
                .with_drain_timeout(Duration::from_millis(80))
                .with_drain_log_interval(Duration::from_millis(20)),
        );
        let backend = Backend::new("b0", 1);
        backend.active_conns.store(3, Ordering::Relaxed);
        pool.add(backend);

        pool.drain_backend("b0").expect("drain should start");

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if pool.get_backends_status().is_empty() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("backend should be force removed after drain timeout");
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn test_fnv1a_determinism(data in any::<Vec<u8>>()) {
            let h1 = fnv1a_hash(&data);
            let h2 = fnv1a_hash(&data);
            prop_assert_eq!(h1, h2);
        }

        #[test]
        fn test_weighted_distribution_exact(
            w1 in 1..100u32,
            w2 in 1..100u32,
            runs in 1000..5000usize
        ) {
            let pool = BackendPool::new();
            pool.add(Backend::new("b1", w1));
            pool.add(Backend::new("b2", w2));

            let mut c1 = 0;
            let mut c2 = 0;
            for _ in 0..runs {
                let b = pool.next_weighted_round_robin().unwrap();
                if b.address == "b1" { c1 += 1; }
                else { c2 += 1; }
            }

            let total_weight = w1 + w2;
            let expected_c1 = (w1 as f64 / total_weight as f64) * runs as f64;
            let expected_c2 = (w2 as f64 / total_weight as f64) * runs as f64;

            // Allow for a small error margin due to integer rounding in sequence
            // Since weighted round-robin is a sequence, not random, the error should be < total_weight
            prop_assert!((c1 as f64 - expected_c1).abs() < total_weight as f64);
            prop_assert!((c2 as f64 - expected_c2).abs() < total_weight as f64);
        }
    }
}
