use std::sync::Arc;
use dashmap::DashMap;
use chrono::{DateTime, Utc, Duration};
use thiserror::Error;
use serde::{Deserialize, Serialize};

#[derive(Error, Debug)]
pub enum RateLimiterError {
    #[error("Internal rate limit error: {0}")]
    Internal(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitStatus {
    pub allowed: bool,
    pub limit: u32,
    pub remaining: u32,
    pub reset_at: DateTime<Utc>,
}

/// A high-performance, in-memory Sliding Window Rate Limiter.
pub struct SlidingWindowRateLimiter {
    /// Mapping of key -> list of timestamps (request history).
    storage: Arc<DashMap<String, Vec<DateTime<Utc>>>>,
    /// Time window for calculating the rate limit.
    window: Duration,
}

impl SlidingWindowRateLimiter {
    /// Create a new rate limiter with a 60-second window by default.
    pub fn new() -> Self {
        Self {
            storage: Arc::new(DashMap::new()),
            window: Duration::seconds(60),
        }
    }

    /// Check if a request for the given key is allowed.
    /// `limit` is the maximum number of requests allowed in the 60s window.
    pub async fn check(&self, key: &str, limit: u32) -> Result<RateLimitStatus, RateLimiterError> {
        let now = Utc::now();
        let cutoff = now - self.window;

        // Entry for this key
        let mut entry = self.storage.entry(key.to_string()).or_insert_with(Vec::new);

        // Remove old entries (Sliding Window logic)
        entry.retain(|&ts| ts > cutoff);

        let allowed = entry.len() < limit as usize;
        if allowed {
            entry.push(now);
        }

        let remaining = if allowed {
             (limit as usize).saturating_sub(entry.len())
        } else {
            0
        };

        // Reset point is approximately 60s from the oldest entry or 60s from now if empty
        let reset_at = if let Some(oldest) = entry.first() {
            *oldest + self.window
        } else {
            now + self.window
        };

        Ok(RateLimitStatus {
            allowed,
            limit,
            remaining: remaining as u32,
            reset_at,
        })
    }

    /// Background task to clean up expired keys from storage.
    pub async fn cleanup(&self) {
        let cutoff = Utc::now() - self.window;
        self.storage.retain(|_, v| {
            v.retain(|&ts| ts > cutoff);
            !v.is_empty()
        });
    }
}

impl Default for SlidingWindowRateLimiter {
    fn default() -> Self {
        Self::new()
    }
}
