use std::net::IpAddr;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use axum::http::HeaderMap;
use governor::clock::{Clock, DefaultClock};
use governor::state::keyed::DefaultKeyedStateStore;
use governor::{Quota, RateLimiter};

// 20 attempts per 15 minutes per IP
const MAX_BURST: u32 = 20;
const REPLENISH_INTERVAL_SECS: u64 = 45; // one token every 45s → ~20/15min

pub struct OAuthRateLimiter {
    limiter: Arc<RateLimiter<IpAddr, DefaultKeyedStateStore<IpAddr>, DefaultClock>>,
}

impl OAuthRateLimiter {
    pub fn new() -> Self {
        let quota = Quota::with_period(Duration::from_secs(REPLENISH_INTERVAL_SECS))
            .expect("non-zero period")
            .allow_burst(NonZeroU32::new(MAX_BURST).expect("non-zero burst"));
        Self {
            limiter: Arc::new(RateLimiter::keyed(quota)),
        }
    }

    pub fn check(&self, headers: &HeaderMap) -> Result<(), u64> {
        let ip = extract_client_ip(headers);
        self.limiter.check_key(&ip).map_err(|not_until| {
            not_until
                .wait_time_from(self.limiter.clock().now())
                .as_secs()
                .max(1)
        })
    }
}

fn extract_client_ip(headers: &HeaderMap) -> IpAddr {
    // CF-Connecting-IP is set by Cloudflare and is the real client IP
    if let Some(ip) = headers
        .get("cf-connecting-ip")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse().ok())
    {
        return ip;
    }

    // X-Forwarded-For: take the first (leftmost) address
    if let Some(ip) = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .and_then(|s| s.trim().parse().ok())
    {
        return ip;
    }

    tracing::warn!("could not extract client IP for rate limiting; using fallback key");
    IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED)
}
