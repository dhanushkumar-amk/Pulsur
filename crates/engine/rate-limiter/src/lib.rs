use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::put,
    Json, Router,
};
use dashmap::DashMap;
#[cfg(not(feature = "noop"))]
use napi_derive::napi;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::warn;

const REDIS_TOKEN_BUCKET_LUA: &str = r#"
local key = KEYS[1]
local now_ms = tonumber(ARGV[1])
local capacity = tonumber(ARGV[2])
local refill_rate = tonumber(ARGV[3])
local requested = tonumber(ARGV[4])

local tokens = tonumber(redis.call('HGET', key, 'tokens'))
local last_refill = tonumber(redis.call('HGET', key, 'last_refill'))

if not tokens then
  tokens = capacity
end

if not last_refill then
  last_refill = now_ms
end

local elapsed = math.max(0, now_ms - last_refill) / 1000.0
tokens = math.min(capacity, tokens + (refill_rate * elapsed))

local allowed = 0
if tokens >= requested then
  tokens = tokens - requested
  allowed = 1
end

redis.call('HSET', key, 'tokens', tokens, 'last_refill', now_ms)

local ttl_ms = math.ceil((capacity / refill_rate) * 1000)
if ttl_ms < 1000 then
  ttl_ms = 1000
end
redis.call('PEXPIRE', key, ttl_ms)

local retry_after_ms = 0
if allowed == 0 then
  local missing = requested - tokens
  retry_after_ms = math.ceil((missing / refill_rate) * 1000)
end

local reset_after_ms = math.ceil(((capacity - tokens) / refill_rate) * 1000)

return {allowed, tokens, retry_after_ms, reset_after_ms}
"#;

#[derive(Error, Debug)]
pub enum RateLimiterError {
    #[error("invalid rate limiter configuration")]
    InvalidConfig,
    #[error("rate limiter mutex poisoned")]
    Poisoned,
    #[error("redis error: {0}")]
    Redis(String),
}

#[derive(Debug, Clone)]
pub struct TokenBucket {
    pub tokens: f64,
    pub capacity: f64,
    pub refill_rate: f64,
    pub last_refill: Instant,
}

impl TokenBucket {
    pub fn new(capacity: f64, refill_rate: f64) -> Result<Self, RateLimiterError> {
        validate_token_bucket(capacity, refill_rate)?;

        Ok(Self {
            tokens: capacity,
            capacity,
            refill_rate,
            last_refill: Instant::now(),
        })
    }

    pub fn try_consume(&mut self, n: f64) -> bool {
        self.try_consume_at(n, Instant::now())
    }

    pub fn try_consume_at(&mut self, n: f64, now: Instant) -> bool {
        if n <= 0.0 || !n.is_finite() {
            return true;
        }

        self.refill_at(now);

        if self.tokens >= n {
            self.tokens -= n;
            true
        } else {
            false
        }
    }

    pub fn refill(&mut self) {
        self.refill_at(Instant::now());
    }

    pub fn refill_at(&mut self, now: Instant) {
        let elapsed_secs = now.duration_since(self.last_refill).as_secs_f64();
        self.tokens = (self.tokens + self.refill_rate * elapsed_secs).min(self.capacity);
        self.last_refill = now;
    }

    pub fn remaining(&self) -> f64 {
        self.tokens.max(0.0)
    }

    pub fn retry_after(&self, requested_tokens: f64) -> Duration {
        if requested_tokens <= self.tokens {
            return Duration::ZERO;
        }

        let missing = requested_tokens - self.tokens;
        Duration::from_secs_f64((missing / self.refill_rate).max(0.0))
    }

    pub fn reset_after(&self) -> Duration {
        let missing = (self.capacity - self.tokens).max(0.0);
        Duration::from_secs_f64((missing / self.refill_rate).max(0.0))
    }
}

#[derive(Debug, Clone)]
pub struct SlidingWindowLog {
    pub requests: VecDeque<Instant>,
    pub window_duration: Duration,
    pub max_requests: usize,
}

impl SlidingWindowLog {
    pub fn new(window_duration: Duration, max_requests: usize) -> Result<Self, RateLimiterError> {
        if window_duration.is_zero() || max_requests == 0 {
            return Err(RateLimiterError::InvalidConfig);
        }

        Ok(Self {
            requests: VecDeque::new(),
            window_duration,
            max_requests,
        })
    }

    pub fn allow(&mut self) -> bool {
        self.allow_at(Instant::now())
    }

    pub fn allow_at(&mut self, now: Instant) -> bool {
        self.drain_expired(now);

        if self.requests.len() >= self.max_requests {
            return false;
        }

        self.requests.push_back(now);
        true
    }

    pub fn drain_expired(&mut self, now: Instant) {
        while let Some(oldest) = self.requests.front() {
            if now.duration_since(*oldest) >= self.window_duration {
                self.requests.pop_front();
            } else {
                break;
            }
        }
    }

    pub fn remaining(&self) -> u64 {
        self.max_requests.saturating_sub(self.requests.len()) as u64
    }

    pub fn retry_after(&self, now: Instant) -> Duration {
        if self.requests.len() < self.max_requests {
            return Duration::ZERO;
        }

        self.requests
            .front()
            .map(|oldest| {
                self.window_duration
                    .saturating_sub(now.duration_since(*oldest))
            })
            .unwrap_or(Duration::ZERO)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RateLimitHeaders {
    pub retry_after: u64,
    pub x_rate_limit_limit: u64,
    pub x_rate_limit_remaining: u64,
    pub x_rate_limit_reset: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RateLimitStatus {
    pub allowed: bool,
    pub limit: u64,
    pub remaining: u64,
    pub retry_after_secs: u64,
    pub reset_after_secs: u64,
}

impl RateLimitStatus {
    pub fn headers(&self) -> RateLimitHeaders {
        RateLimitHeaders {
            retry_after: self.retry_after_secs,
            x_rate_limit_limit: self.limit,
            x_rate_limit_remaining: self.remaining,
            x_rate_limit_reset: self.reset_after_secs,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WindowType {
    TokenBucket,
    SlidingWindow,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RateLimiterConfig {
    pub window_type: WindowType,
    pub redis_url: String,
}

impl Default for RateLimiterConfig {
    fn default() -> Self {
        Self {
            window_type: WindowType::TokenBucket,
            redis_url: "redis://localhost:6379".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LimitRule {
    pub capacity: f64,
    pub refill_rate: f64,
}

impl LimitRule {
    pub fn new(capacity: f64, refill_rate: f64) -> Result<Self, RateLimiterError> {
        validate_token_bucket(capacity, refill_rate)?;
        Ok(Self {
            capacity,
            refill_rate,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct MultiTierRateLimitConfig {
    pub global_limit: Option<LimitRule>,
    pub user_limit: Option<LimitRule>,
    pub endpoint_limits: HashMap<String, LimitRule>,
    pub ip_allowlist: HashSet<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RateLimitRequest {
    pub user_id: Option<String>,
    pub endpoint: Option<String>,
    pub ip_address: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TierStatus {
    pub tier: String,
    pub key: String,
    pub status: RateLimitStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MultiTierRateLimitResult {
    pub allowed: bool,
    pub bypassed: bool,
    pub denied_by: Option<String>,
    pub tiers: Vec<TierStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UpdateRateLimitRuleRequest {
    pub capacity: f64,
    pub refill_rate: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UpdateRateLimitRuleResponse {
    pub key: String,
    pub rule: LimitRule,
}

#[derive(Default)]
struct InMemoryBucketStore {
    buckets: DashMap<String, Mutex<TokenBucket>>,
}

impl InMemoryBucketStore {
    fn check_key(
        &self,
        key: &str,
        tokens: f64,
        rule: &LimitRule,
    ) -> Result<RateLimitStatus, RateLimiterError> {
        let bucket_mutex = self.buckets.entry(key.to_string()).or_insert_with(|| {
            Mutex::new(
                TokenBucket::new(rule.capacity, rule.refill_rate)
                    .expect("validated token bucket config"),
            )
        });

        let mut bucket = bucket_mutex
            .lock()
            .map_err(|_| RateLimiterError::Poisoned)?;

        if (bucket.capacity - rule.capacity).abs() > f64::EPSILON
            || (bucket.refill_rate - rule.refill_rate).abs() > f64::EPSILON
        {
            *bucket = TokenBucket::new(rule.capacity, rule.refill_rate)?;
        }

        let allowed = bucket.try_consume(tokens);
        let remaining = bucket.remaining().floor() as u64;
        let retry_after_secs = if allowed {
            0
        } else {
            ceil_seconds(bucket.retry_after(tokens))
        };
        let reset_after_secs = ceil_seconds(bucket.reset_after());

        Ok(RateLimitStatus {
            allowed,
            limit: rule.capacity.floor() as u64,
            remaining,
            retry_after_secs,
            reset_after_secs,
        })
    }
}

pub struct TokenBucketRateLimiter {
    store: Arc<InMemoryBucketStore>,
    capacity: f64,
    refill_rate: f64,
}

impl TokenBucketRateLimiter {
    pub fn new(capacity: f64, refill_rate: f64) -> Result<Self, RateLimiterError> {
        validate_token_bucket(capacity, refill_rate)?;

        Ok(Self {
            store: Arc::new(InMemoryBucketStore::default()),
            capacity,
            refill_rate,
        })
    }

    pub fn check_key(&self, key: &str, tokens: f64) -> Result<RateLimitStatus, RateLimiterError> {
        self.store.check_key(
            key,
            tokens,
            &LimitRule {
                capacity: self.capacity,
                refill_rate: self.refill_rate,
            },
        )
    }

    pub fn extract_client_key(
        api_key: Option<&str>,
        jwt_sub: Option<&str>,
        ip_address: Option<&str>,
    ) -> Option<String> {
        api_key
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .or_else(|| {
                jwt_sub
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
            })
            .or_else(|| {
                ip_address
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
            })
    }

    pub fn buckets_len(&self) -> usize {
        self.store.buckets.len()
    }
}

impl Default for TokenBucketRateLimiter {
    fn default() -> Self {
        Self::new(100.0, 100.0).expect("default token bucket config should be valid")
    }
}

pub struct SlidingWindowRateLimiter {
    logs: Arc<DashMap<String, Mutex<SlidingWindowLog>>>,
    window_duration: Duration,
    max_requests: usize,
}

impl SlidingWindowRateLimiter {
    pub fn new(window_duration: Duration, max_requests: usize) -> Result<Self, RateLimiterError> {
        SlidingWindowLog::new(window_duration, max_requests)?;

        Ok(Self {
            logs: Arc::new(DashMap::new()),
            window_duration,
            max_requests,
        })
    }

    pub fn check_key(&self, key: &str) -> Result<RateLimitStatus, RateLimiterError> {
        self.check_key_at(key, Instant::now())
    }

    pub fn check_key_at(
        &self,
        key: &str,
        now: Instant,
    ) -> Result<RateLimitStatus, RateLimiterError> {
        let log_mutex = self.logs.entry(key.to_string()).or_insert_with(|| {
            Mutex::new(
                SlidingWindowLog::new(self.window_duration, self.max_requests)
                    .expect("validated sliding window config"),
            )
        });

        let mut log = log_mutex.lock().map_err(|_| RateLimiterError::Poisoned)?;
        let allowed = log.allow_at(now);
        let remaining = log.remaining();
        let retry_after_secs = if allowed {
            0
        } else {
            ceil_seconds(log.retry_after(now))
        };

        Ok(RateLimitStatus {
            allowed,
            limit: self.max_requests as u64,
            remaining,
            retry_after_secs,
            reset_after_secs: retry_after_secs,
        })
    }

    pub fn logs_len(&self) -> usize {
        self.logs.len()
    }
}

#[cfg(not(feature = "noop"))]
#[derive(Clone)]
#[napi(object)]
pub struct JsRateLimitResult {
    pub allowed: bool,
    pub limit: u32,
    pub remaining: u32,
    pub retry_after_secs: u32,
    pub reset_after_secs: u32,
}

#[cfg(not(feature = "noop"))]
impl From<RateLimitStatus> for JsRateLimitResult {
    fn from(value: RateLimitStatus) -> Self {
        Self {
            allowed: value.allowed,
            limit: value.limit as u32,
            remaining: value.remaining as u32,
            retry_after_secs: value.retry_after_secs as u32,
            reset_after_secs: value.reset_after_secs as u32,
        }
    }
}

#[cfg(not(feature = "noop"))]
#[napi]
pub struct JsSlidingWindowLimiter {
    inner: Arc<SlidingWindowRateLimiter>,
}

#[cfg(not(feature = "noop"))]
#[napi]
impl JsSlidingWindowLimiter {
    #[napi(factory)]
    pub fn create_limiter(max_requests: u32, window_ms: u32) -> napi::Result<Self> {
        let limiter = SlidingWindowRateLimiter::new(
            Duration::from_millis(u64::from(window_ms.max(1))),
            max_requests.max(1) as usize,
        )
        .map_err(|err| napi::Error::from_reason(err.to_string()))?;

        Ok(Self {
            inner: Arc::new(limiter),
        })
    }

    #[napi]
    pub async fn check_limit(&self, key: String) -> napi::Result<JsRateLimitResult> {
        self.inner
            .check_key(&key)
            .map(JsRateLimitResult::from)
            .map_err(|err| napi::Error::from_reason(err.to_string()))
    }

    #[napi]
    pub fn size(&self) -> u32 {
        self.inner.logs_len() as u32
    }
}

#[cfg(not(feature = "noop"))]
#[napi]
pub fn create_limiter(max_requests: u32, window_ms: u32) -> napi::Result<JsSlidingWindowLimiter> {
    JsSlidingWindowLimiter::create_limiter(max_requests, window_ms)
}

impl Default for SlidingWindowRateLimiter {
    fn default() -> Self {
        Self::new(Duration::from_secs(60), 100)
            .expect("default sliding window config should be valid")
    }
}

pub struct DistributedTokenBucketRateLimiter {
    redis_client: redis::Client,
    script_sha: Arc<RwLock<String>>,
    fallback: Arc<TokenBucketRateLimiter>,
    redis_url: String,
    capacity: f64,
    refill_rate: f64,
}

impl DistributedTokenBucketRateLimiter {
    pub async fn new(
        redis_url: &str,
        capacity: f64,
        refill_rate: f64,
    ) -> Result<Self, RateLimiterError> {
        validate_token_bucket(capacity, refill_rate)?;
        let redis_client = redis::Client::open(redis_url)
            .map_err(|err| RateLimiterError::Redis(err.to_string()))?;
        let script_sha = load_lua_script(&redis_client).await?;

        Ok(Self {
            redis_client,
            script_sha: Arc::new(RwLock::new(script_sha)),
            fallback: Arc::new(TokenBucketRateLimiter::new(capacity, refill_rate)?),
            redis_url: redis_url.to_string(),
            capacity,
            refill_rate,
        })
    }

    pub async fn check_key(
        &self,
        key: &str,
        tokens: f64,
    ) -> Result<RateLimitStatus, RateLimiterError> {
        let now_ms = unix_now_millis();
        let sha = self
            .script_sha
            .read()
            .map_err(|_| RateLimiterError::Poisoned)?
            .clone();

        match self.evalsha_with_retry(&sha, key, tokens, now_ms).await {
            Ok(status) => Ok(status),
            Err(err) => {
                warn!(
                    "redis rate limiter unavailable at {}; falling back to in-memory limiter: {}",
                    self.redis_url, err
                );
                self.fallback.check_key(key, tokens)
            }
        }
    }

    async fn evalsha_with_retry(
        &self,
        sha: &str,
        key: &str,
        tokens: f64,
        now_ms: u64,
    ) -> Result<RateLimitStatus, String> {
        match self.evalsha_with_sha(sha, key, tokens, now_ms).await {
            Ok(status) => Ok(status),
            Err(err) if err.contains("NOSCRIPT") => {
                let refreshed = load_lua_script(&self.redis_client)
                    .await
                    .map_err(|load_err| load_err.to_string())?;
                *self
                    .script_sha
                    .write()
                    .map_err(|_| "poisoned script sha".to_string())? = refreshed.clone();
                self.evalsha_with_sha(&refreshed, key, tokens, now_ms).await
            }
            Err(err) => Err(err),
        }
    }

    async fn evalsha_with_sha(
        &self,
        sha: &str,
        key: &str,
        tokens: f64,
        now_ms: u64,
    ) -> Result<RateLimitStatus, String> {
        let mut conn = self
            .redis_client
            .get_multiplexed_async_connection()
            .await
            .map_err(|err| err.to_string())?;

        let result: Vec<f64> = redis::cmd("EVALSHA")
            .arg(sha)
            .arg(1)
            .arg(key)
            .arg(now_ms)
            .arg(self.capacity)
            .arg(self.refill_rate)
            .arg(tokens)
            .query_async(&mut conn)
            .await
            .map_err(|err| err.to_string())?;

        if result.len() != 4 {
            return Err("unexpected lua result length".to_string());
        }

        Ok(RateLimitStatus {
            allowed: result[0] >= 1.0,
            limit: self.capacity.floor() as u64,
            remaining: result[1].floor().max(0.0) as u64,
            retry_after_secs: ceil_seconds(Duration::from_secs_f64((result[2] / 1000.0).max(0.0))),
            reset_after_secs: ceil_seconds(Duration::from_secs_f64((result[3] / 1000.0).max(0.0))),
        })
    }
}

pub struct MultiTierRateLimiter {
    config: Arc<RwLock<MultiTierRateLimitConfig>>,
    dynamic_rules: Arc<DashMap<String, LimitRule>>,
    store: Arc<InMemoryBucketStore>,
}

impl MultiTierRateLimiter {
    pub fn new(config: MultiTierRateLimitConfig) -> Self {
        Self {
            config: Arc::new(RwLock::new(config)),
            dynamic_rules: Arc::new(DashMap::new()),
            store: Arc::new(InMemoryBucketStore::default()),
        }
    }

    pub fn check(
        &self,
        request: &RateLimitRequest,
    ) -> Result<MultiTierRateLimitResult, RateLimiterError> {
        let config = self
            .config
            .read()
            .map_err(|_| RateLimiterError::Poisoned)?
            .clone();

        if request
            .ip_address
            .as_deref()
            .is_some_and(|ip| config.ip_allowlist.contains(ip))
        {
            return Ok(MultiTierRateLimitResult {
                allowed: true,
                bypassed: true,
                denied_by: None,
                tiers: Vec::new(),
            });
        }

        let mut tiers = Vec::new();
        let mut denied_by = None;

        if let Some(rule) = self.rule_for("global", config.global_limit) {
            let key = "global".to_string();
            let status = self.store.check_key(&key, 1.0, &rule)?;
            if !status.allowed {
                denied_by = Some("global".to_string());
            }
            tiers.push(TierStatus {
                tier: "global".to_string(),
                key,
                status,
            });
        }

        if let (Some(user_id), Some(rule)) =
            (&request.user_id, self.rule_for("user", config.user_limit))
        {
            let key = format!("user:{user_id}");
            let status = self.store.check_key(&key, 1.0, &rule)?;
            if denied_by.is_none() && !status.allowed {
                denied_by = Some("user".to_string());
            }
            tiers.push(TierStatus {
                tier: "user".to_string(),
                key,
                status,
            });
        }

        if let Some(endpoint) = &request.endpoint {
            let dynamic_key = format!("endpoint:{endpoint}");
            let endpoint_rule = self
                .dynamic_rules
                .get(&dynamic_key)
                .map(|entry| entry.clone())
                .or_else(|| config.endpoint_limits.get(endpoint).cloned());

            if let Some(rule) = endpoint_rule {
                let status = self.store.check_key(&dynamic_key, 1.0, &rule)?;
                if denied_by.is_none() && !status.allowed {
                    denied_by = Some("endpoint".to_string());
                }
                tiers.push(TierStatus {
                    tier: "endpoint".to_string(),
                    key: dynamic_key,
                    status,
                });
            }
        }

        Ok(MultiTierRateLimitResult {
            allowed: denied_by.is_none(),
            bypassed: false,
            denied_by,
            tiers,
        })
    }

    pub fn update_rule(&self, key: &str, rule: LimitRule) {
        self.dynamic_rules.insert(key.to_string(), rule);
    }

    pub fn admin_router(self: Arc<Self>) -> Router {
        Router::new()
            .route("/rate-limits/:key", put(update_rate_limit_rule))
            .with_state(self)
    }

    fn rule_for(&self, tier: &str, base_rule: Option<LimitRule>) -> Option<LimitRule> {
        self.dynamic_rules
            .get(tier)
            .map(|entry| entry.clone())
            .or(base_rule)
    }
}

async fn update_rate_limit_rule(
    Path(key): Path<String>,
    State(limiter): State<Arc<MultiTierRateLimiter>>,
    Json(payload): Json<UpdateRateLimitRuleRequest>,
) -> impl IntoResponse {
    match LimitRule::new(payload.capacity, payload.refill_rate) {
        Ok(rule) => {
            limiter.update_rule(&key, rule.clone());
            (
                StatusCode::OK,
                Json(UpdateRateLimitRuleResponse { key, rule }),
            )
                .into_response()
        }
        Err(_) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "invalid rule configuration" })),
        )
            .into_response(),
    }
}

async fn load_lua_script(client: &redis::Client) -> Result<String, RateLimiterError> {
    let mut conn = client
        .get_multiplexed_async_connection()
        .await
        .map_err(|err| RateLimiterError::Redis(err.to_string()))?;

    redis::cmd("SCRIPT")
        .arg("LOAD")
        .arg(REDIS_TOKEN_BUCKET_LUA)
        .query_async(&mut conn)
        .await
        .map_err(|err| RateLimiterError::Redis(err.to_string()))
}

fn validate_token_bucket(capacity: f64, refill_rate: f64) -> Result<(), RateLimiterError> {
    if capacity <= 0.0 || refill_rate <= 0.0 || !capacity.is_finite() || !refill_rate.is_finite() {
        Err(RateLimiterError::InvalidConfig)
    } else {
        Ok(())
    }
}

fn ceil_seconds(duration: Duration) -> u64 {
    duration.as_secs() + u64::from(duration.subsec_nanos() > 0)
}

fn unix_now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;
    use std::process::{Child, Command, Stdio};
    use std::thread;

    use axum::body::{to_bytes, Body};
    use http::{Request, StatusCode};
    use proptest::prelude::*;
    use tower::ServiceExt;

    use super::*;

    #[test]
    fn token_bucket_consumes_until_empty() {
        let mut bucket = TokenBucket::new(5.0, 1.0).expect("bucket should build");

        assert!(bucket.try_consume(3.0));
        assert_eq!(bucket.remaining().floor() as u64, 2);
        assert!(bucket.try_consume(2.0));
        assert!(!bucket.try_consume(1.0));
    }

    #[test]
    fn token_bucket_refills_over_time() {
        let mut bucket = TokenBucket::new(2.0, 10.0).expect("bucket should build");
        assert!(bucket.try_consume(2.0));
        assert_eq!(bucket.remaining().floor() as u64, 0);

        thread::sleep(Duration::from_millis(150));
        assert!(bucket.try_consume(1.0));
    }

    #[test]
    fn extract_client_key_prefers_api_key_then_jwt_then_ip() {
        assert_eq!(
            TokenBucketRateLimiter::extract_client_key(
                Some("api-123"),
                Some("user-123"),
                Some("10.0.0.1")
            ),
            Some("api-123".to_string())
        );
        assert_eq!(
            TokenBucketRateLimiter::extract_client_key(None, Some("user-123"), Some("10.0.0.1")),
            Some("user-123".to_string())
        );
        assert_eq!(
            TokenBucketRateLimiter::extract_client_key(None, None, Some("10.0.0.1")),
            Some("10.0.0.1".to_string())
        );
    }

    #[test]
    fn denied_requests_report_retry_headers() {
        let limiter = TokenBucketRateLimiter::new(2.0, 1.0).expect("limiter should build");

        assert!(
            limiter
                .check_key("client-a", 1.0)
                .expect("check should work")
                .allowed
        );
        assert!(
            limiter
                .check_key("client-a", 1.0)
                .expect("check should work")
                .allowed
        );

        let blocked = limiter
            .check_key("client-a", 1.0)
            .expect("check should work");
        assert!(!blocked.allowed);
        assert_eq!(blocked.limit, 2);
        assert_eq!(blocked.remaining, 0);
        assert!(blocked.retry_after_secs >= 1);

        let headers = blocked.headers();
        assert_eq!(headers.x_rate_limit_limit, 2);
        assert_eq!(headers.x_rate_limit_remaining, 0);
        assert!(headers.retry_after >= 1);
    }

    #[test]
    fn sliding_window_rejects_when_window_is_full() {
        let limiter = SlidingWindowRateLimiter::new(Duration::from_secs(60), 2)
            .expect("limiter should build");

        assert!(
            limiter
                .check_key("client-a")
                .expect("check should work")
                .allowed
        );
        assert!(
            limiter
                .check_key("client-a")
                .expect("check should work")
                .allowed
        );

        let blocked = limiter.check_key("client-a").expect("check should work");
        assert!(!blocked.allowed);
        assert_eq!(blocked.limit, 2);
        assert_eq!(blocked.remaining, 0);
        assert!(blocked.retry_after_secs >= 1);
    }

    #[test]
    fn sliding_window_drains_expired_requests_before_allowing() {
        let mut log =
            SlidingWindowLog::new(Duration::from_millis(50), 2).expect("log should build");
        let start = Instant::now();

        assert!(log.allow_at(start));
        assert!(log.allow_at(start + Duration::from_millis(10)));
        assert!(!log.allow_at(start + Duration::from_millis(20)));
        assert!(log.allow_at(start + Duration::from_millis(60)));
    }

    #[test]
    fn sliding_window_is_more_accurate_than_token_bucket_at_boundary() {
        let mut bucket = TokenBucket::new(5.0, 5.0).expect("bucket should build");
        let start = Instant::now();
        bucket.tokens = 0.0;
        bucket.last_refill = start;

        let token_bucket_allows_after_partial_refill =
            bucket.try_consume_at(1.0, start + Duration::from_millis(200));

        let mut sliding =
            SlidingWindowLog::new(Duration::from_secs(1), 5).expect("log should build");
        for offset_ms in [0_u64, 50, 100, 150, 190] {
            assert!(sliding.allow_at(start + Duration::from_millis(offset_ms)));
        }
        let sliding_window_allows_at_same_boundary =
            sliding.allow_at(start + Duration::from_millis(200));

        assert!(token_bucket_allows_after_partial_refill);
        assert!(!sliding_window_allows_at_same_boundary);
    }

    #[test]
    fn user_limit_can_reject_while_global_limit_still_has_capacity() {
        let limiter = MultiTierRateLimiter::new(MultiTierRateLimitConfig {
            global_limit: Some(LimitRule::new(10.0, 0.000_001).expect("rule should build")),
            user_limit: Some(LimitRule::new(2.0, 0.000_001).expect("rule should build")),
            endpoint_limits: HashMap::new(),
            ip_allowlist: HashSet::new(),
        });

        let request = RateLimitRequest {
            user_id: Some("user-1".to_string()),
            endpoint: None,
            ip_address: Some("10.0.0.5".to_string()),
        };

        assert!(limiter.check(&request).expect("check should work").allowed);
        assert!(limiter.check(&request).expect("check should work").allowed);

        let blocked = limiter.check(&request).expect("check should work");
        assert!(!blocked.allowed);
        assert_eq!(blocked.denied_by.as_deref(), Some("user"));

        let global_tier = blocked
            .tiers
            .iter()
            .find(|tier| tier.tier == "global")
            .expect("global tier should exist");
        let user_tier = blocked
            .tiers
            .iter()
            .find(|tier| tier.tier == "user")
            .expect("user tier should exist");

        assert!(global_tier.status.allowed);
        assert!(!user_tier.status.allowed);
    }

    #[tokio::test]
    async fn admin_api_updates_dynamic_rule() {
        let limiter = Arc::new(MultiTierRateLimiter::new(
            MultiTierRateLimitConfig::default(),
        ));

        let response = limiter
            .clone()
            .admin_router()
            .oneshot(
                Request::builder()
                    .uri("/rate-limits/endpoint:upload")
                    .method("PUT")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&UpdateRateLimitRuleRequest {
                            capacity: 3.0,
                            refill_rate: 1.0,
                        })
                        .expect("request body should serialize"),
                    ))
                    .expect("request should build"),
            )
            .await
            .expect("router should respond");

        assert_eq!(response.status(), StatusCode::OK);

        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should be readable");
        let payload: UpdateRateLimitRuleResponse =
            serde_json::from_slice(&body).expect("response should deserialize");

        assert_eq!(payload.key, "endpoint:upload");
        assert_eq!(
            payload.rule,
            LimitRule::new(3.0, 1.0).expect("rule should build")
        );
    }

    proptest! {
        #[test]
        fn token_bucket_never_exceeds_capacity(
            capacity in 1u32..50,
            refill_rate in 1u32..50,
            spend in 1u32..50
        ) {
            let mut bucket = TokenBucket::new(capacity as f64, refill_rate as f64)
                .expect("bucket should build");

            for _ in 0..100 {
                let _ = bucket.try_consume(spend.min(capacity) as f64);
                bucket.refill();
                prop_assert!(bucket.tokens <= bucket.capacity + f64::EPSILON);
                prop_assert!(bucket.tokens >= 0.0);
            }
        }

        #[test]
        fn sequential_requests_never_allow_more_than_capacity_without_wait(
            capacity in 1u32..20
        ) {
            let limiter = TokenBucketRateLimiter::new(capacity as f64, 0.000_001)
                .expect("limiter should build");
            let key = "client-seq";
            let mut allowed = 0u32;

            for _ in 0..=capacity {
                if limiter.check_key(key, 1.0).expect("check should work").allowed {
                    allowed += 1;
                }
            }
            prop_assert!(allowed <= capacity);
        }
    }

    #[test]
    fn concurrent_checks_do_not_exceed_capacity_when_no_refill_happens() {
        let limiter =
            Arc::new(TokenBucketRateLimiter::new(10.0, 0.000_001).expect("limiter should build"));
        let mut threads = Vec::new();

        for _ in 0..50 {
            let limiter = Arc::clone(&limiter);
            threads.push(thread::spawn(move || {
                limiter
                    .check_key("shared-client", 1.0)
                    .expect("check should work")
                    .allowed
            }));
        }

        let allowed = threads
            .into_iter()
            .map(|handle| handle.join().expect("thread should finish"))
            .filter(|allowed| *allowed)
            .count();

        assert!(allowed <= 10);
    }

    #[tokio::test]
    async fn distributed_limiter_falls_back_to_in_memory_when_redis_is_unreachable() {
        let limiter = DistributedTokenBucketRateLimiter {
            redis_client: redis::Client::open("redis://127.0.0.1:1")
                .expect("redis client should parse"),
            script_sha: Arc::new(RwLock::new("missing".to_string())),
            fallback: Arc::new(
                TokenBucketRateLimiter::new(2.0, 0.000_001).expect("limiter should build"),
            ),
            redis_url: "redis://127.0.0.1:1".to_string(),
            capacity: 2.0,
            refill_rate: 0.000_001,
        };

        assert!(
            limiter
                .check_key("shared", 1.0)
                .await
                .expect("check should work")
                .allowed
        );
        assert!(
            limiter
                .check_key("shared", 1.0)
                .await
                .expect("check should work")
                .allowed
        );
        assert!(
            !limiter
                .check_key("shared", 1.0)
                .await
                .expect("check should work")
                .allowed
        );
    }

    #[tokio::test]
    async fn distributed_limit_is_respected_across_three_instances_when_redis_is_available() {
        let Some((mut redis_child, port)) = spawn_redis_server().await else {
            return;
        };

        let redis_url = format!("redis://127.0.0.1:{port}");

        let limiter_a = DistributedTokenBucketRateLimiter::new(&redis_url, 5.0, 0.000_001)
            .await
            .expect("limiter should connect");
        let limiter_b = DistributedTokenBucketRateLimiter::new(&redis_url, 5.0, 0.000_001)
            .await
            .expect("limiter should connect");
        let limiter_c = DistributedTokenBucketRateLimiter::new(&redis_url, 5.0, 0.000_001)
            .await
            .expect("limiter should connect");

        let mut allowed = 0usize;
        for limiter in [
            &limiter_a, &limiter_b, &limiter_c, &limiter_a, &limiter_b, &limiter_c,
        ] {
            if limiter
                .check_key("global-client", 1.0)
                .await
                .expect("redis check should work")
                .allowed
            {
                allowed += 1;
            }
        }

        let _ = redis_child.kill();
        let _ = redis_child.wait();

        assert_eq!(allowed, 5);
    }

    async fn spawn_redis_server() -> Option<(Child, u16)> {
        let port = next_open_port()?;

        let child = Command::new("redis-server")
            .arg("--save")
            .arg("")
            .arg("--appendonly")
            .arg("no")
            .arg("--port")
            .arg(port.to_string())
            .arg("--bind")
            .arg("127.0.0.1")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;

        tokio::time::sleep(Duration::from_millis(300)).await;
        Some((child, port))
    }

    fn next_open_port() -> Option<u16> {
        TcpListener::bind("127.0.0.1:0")
            .ok()
            .and_then(|listener| listener.local_addr().ok().map(|addr| addr.port()))
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn test_token_bucket_refill_properties(
            capacity in 1.0..1000.0,
            refill_rate in 0.1..100.0,
            wait_time_ms in 0..1000u64
        ) {
            let mut bucket = TokenBucket::new(capacity, refill_rate).unwrap();
            let start_time = Instant::now();

            // Consume everything
            bucket.try_consume(capacity);
            prop_assert!(bucket.remaining() < 1.0);

            // Wait and refill
            let now = start_time + Duration::from_millis(wait_time_ms);
            bucket.refill_at(now);

            // Check that we didn't exceed capacity
            prop_assert!(bucket.remaining() <= capacity);

            // Check that we refilled approximately the right amount
            let expected_refill = ((wait_time_ms as f64 / 1000.0) * refill_rate).min(capacity);
            let actual_refill = bucket.remaining();
            prop_assert!(actual_refill >= expected_refill - 0.1);
        }

        #[test]
        fn test_sliding_window_log_properties(
            window_ms in 10..1000u64,
            max_reqs in 1..100usize,
            num_reqs in 1..200usize
        ) {
            let window = Duration::from_millis(window_ms);
            let mut log = SlidingWindowLog::new(window, max_reqs).unwrap();
            let start = Instant::now();

            let mut allowed_count = 0;
            for _ in 0..num_reqs {
                if log.allow_at(start) {
                    allowed_count += 1;
                }
            }

            // At the same instant, we should never allow more than max_reqs
            prop_assert!(allowed_count <= max_reqs);
            prop_assert_eq!(allowed_count, num_reqs.min(max_reqs));
        }
    }
}
