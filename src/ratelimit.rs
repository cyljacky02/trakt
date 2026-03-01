use std::net::IpAddr;
use std::num::NonZeroU32;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use governor::{DefaultDirectRateLimiter, DefaultKeyedRateLimiter, Quota, RateLimiter as GovRateLimiter};
use serde::{Deserialize, Serialize};

/// Per-IP domain-specific state (session tracking and bans).
///
/// All fields are plain types; synchronization is provided by the containing
/// `DashMap` shard locks. Reads use shared locks, writes use exclusive locks.
struct IpState {
    /// Number of active sessions for this IP.
    session_count: u32,
    /// Consecutive handshake failures.
    handshake_failures: u32,
    /// Temporary ban expiry time, if currently banned.
    banned_until: Option<Instant>,
}

impl IpState {
    fn new() -> Self {
        Self {
            session_count: 0,
            handshake_failures: 0,
            banned_until: None,
        }
    }

    /// Returns true if this IP is currently banned.
    #[inline]
    fn is_banned(&self) -> bool {
        self.banned_until.is_some_and(|t| Instant::now() < t)
    }
}

/// Converts a `u32` config value to `NonZeroU32`, falling back to 1 if zero.
#[inline]
fn nonzero(value: u32) -> NonZeroU32 {
    NonZeroU32::new(value).unwrap_or(NonZeroU32::MIN)
}

/// GCRA-based rate limiter with domain-specific session and ban tracking.
///
/// Rate checks use the Generic Cell Rate Algorithm (via `governor`) for
/// lock-free, O(1) atomic packet/ping/session rate limiting. Domain state
/// (session counts, handshake failures, temporary bans) is stored in a
/// sharded `DashMap` for minimal contention on the hot path.
pub struct RateLimiter {
    /// GCRA per-IP packet rate limiter (lock-free atomic CAS).
    packet_limiter: DefaultKeyedRateLimiter<IpAddr>,
    /// GCRA per-IP ping rate limiter (lock-free atomic CAS).
    ping_limiter: DefaultKeyedRateLimiter<IpAddr>,
    /// GCRA global new-session rate limiter (lock-free atomic CAS).
    global_session_limiter: DefaultDirectRateLimiter,

    /// Per-IP domain state: session counts, handshake failures, bans.
    ip_state: DashMap<IpAddr, IpState>,

    /// Maximum concurrent sessions per IP.
    session_limit: u32,
    /// Handshake failures before temporary ban.
    max_handshake_failures: u32,
    /// Duration of temporary bans.
    ban_duration: Duration,
    /// Minimum valid packet size in bytes.
    min_packet_size: usize,
    /// Maximum valid packet size in bytes.
    max_packet_size: usize,
    /// Maximum packets per second per connected session.
    session_pps_limit: u32,
}

/// Result of a rate limit check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitResult {
    /// Packet is allowed.
    Allow,
    /// Packet dropped: per-IP pps exceeded.
    IpPpsExceeded,
    /// Packet dropped: IP is temporarily banned.
    IpBanned,
    /// Packet dropped: per-IP ping rate exceeded.
    PingRateExceeded,
    /// Packet dropped: per-IP session limit exceeded.
    SessionLimitExceeded,
    /// Packet dropped: global new-session rate exceeded.
    GlobalSessionRateExceeded,
    /// Packet dropped: packet size out of valid range.
    InvalidPacketSize,
}

impl RateLimiter {
    pub fn new(config: &RateLimitConfig) -> Self {
        Self {
            packet_limiter: GovRateLimiter::keyed(
                Quota::per_second(nonzero(config.per_ip_pps)),
            ),
            ping_limiter: GovRateLimiter::keyed(
                Quota::per_second(nonzero(config.per_ip_ping_pps)),
            ),
            global_session_limiter: GovRateLimiter::direct(
                Quota::per_second(nonzero(config.global_new_sessions_pps)),
            ),
            ip_state: DashMap::new(),
            session_limit: config.per_ip_max_sessions,
            max_handshake_failures: config.max_handshake_failures,
            ban_duration: Duration::from_secs(config.ban_duration_secs),
            min_packet_size: config.min_packet_size,
            max_packet_size: config.max_packet_size,
            session_pps_limit: config.per_session_pps,
        }
    }

    /// Check if a packet from this IP should be allowed.
    ///
    /// Hot path: called for every incoming packet. Uses only a DashMap shared
    /// read lock (for ban check) and a lock-free GCRA atomic CAS (for rate).
    pub fn check_packet(&self, ip: IpAddr, packet_len: usize) -> RateLimitResult {
        if packet_len < self.min_packet_size || packet_len > self.max_packet_size {
            return RateLimitResult::InvalidPacketSize;
        }

        // Ban check (DashMap read lock on shard — concurrent with other reads)
        if let Some(state) = self.ip_state.get(&ip) {
            if state.is_banned() {
                return RateLimitResult::IpBanned;
            }
        }

        // GCRA rate check (lock-free atomic CAS)
        if self.packet_limiter.check_key(&ip).is_err() {
            return RateLimitResult::IpPpsExceeded;
        }

        RateLimitResult::Allow
    }

    /// Check if an unconnected ping from this IP should be allowed.
    pub fn check_ping(&self, ip: IpAddr) -> RateLimitResult {
        // GCRA rate check (lock-free atomic CAS)
        if self.ping_limiter.check_key(&ip).is_err() {
            return RateLimitResult::PingRateExceeded;
        }
        RateLimitResult::Allow
    }

    /// Check if a new session from this IP should be allowed.
    ///
    /// Atomically validates global rate, per-IP ban status, and session limit,
    /// then increments the session counter if allowed.
    pub fn check_new_session(&self, ip: IpAddr) -> RateLimitResult {
        // Global rate check (lock-free atomic CAS)
        if self.global_session_limiter.check().is_err() {
            return RateLimitResult::GlobalSessionRateExceeded;
        }

        // Per-IP checks (DashMap write lock on shard for atomic check-and-increment)
        let mut state = self.ip_state.entry(ip).or_insert_with(IpState::new);

        // Ban check — clear expired bans inline
        if let Some(banned_until) = state.banned_until {
            if Instant::now() < banned_until {
                return RateLimitResult::IpBanned;
            }
            state.banned_until = None;
            state.handshake_failures = 0;
        }

        // Session limit check
        if state.session_count >= self.session_limit {
            return RateLimitResult::SessionLimitExceeded;
        }

        state.session_count += 1;
        RateLimitResult::Allow
    }

    /// Notify that a session for this IP was closed.
    pub fn session_closed(&self, ip: IpAddr) {
        if let Some(mut state) = self.ip_state.get_mut(&ip) {
            state.session_count = state.session_count.saturating_sub(1);
        }
    }

    /// Record a handshake failure for this IP. May trigger a temporary ban.
    pub fn record_handshake_failure(&self, ip: IpAddr) {
        let mut state = self.ip_state.entry(ip).or_insert_with(IpState::new);
        state.handshake_failures += 1;
        if state.handshake_failures >= self.max_handshake_failures {
            state.banned_until = Some(Instant::now() + self.ban_duration);
            tracing::warn!(
                %ip,
                failures = state.handshake_failures,
                ban_secs = self.ban_duration.as_secs(),
                "IP temporarily banned for repeated handshake failures"
            );
        }
    }

    /// Record a successful handshake, resetting failure count.
    pub fn record_handshake_success(&self, ip: IpAddr) {
        if let Some(mut state) = self.ip_state.get_mut(&ip) {
            state.handshake_failures = 0;
        }
    }

    /// Returns the per-session pps limit.
    pub fn session_pps_limit(&self) -> u32 {
        self.session_pps_limit
    }

    /// Periodic cleanup of stale entries (call every ~30s from a background task).
    ///
    /// Evicts expired GCRA states from governor's internal stores and removes
    /// idle per-IP entries from the domain state map.
    pub fn cleanup_stale_entries(&self) {
        // Clean up governor's internal keyed state stores
        self.packet_limiter.retain_recent();
        self.ping_limiter.retain_recent();

        // Clean up our own IP state — keep only entries with active sessions or unexpired bans
        self.ip_state.retain(|_, state| {
            state.session_count > 0
                || state.banned_until.is_some_and(|t| Instant::now() < t)
        });
    }
}

/// Configuration for rate limiting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitConfig {
    /// Maximum packets per second per IP (all packet types).
    #[serde(default = "RateLimitConfig::default_per_ip_pps")]
    pub per_ip_pps: u32,
    /// Maximum concurrent sessions per IP.
    #[serde(default = "RateLimitConfig::default_per_ip_max_sessions")]
    pub per_ip_max_sessions: u32,
    /// Maximum new sessions per second globally.
    #[serde(default = "RateLimitConfig::default_global_new_sessions_pps")]
    pub global_new_sessions_pps: u32,
    /// Maximum unconnected pings per second per IP.
    #[serde(default = "RateLimitConfig::default_per_ip_ping_pps")]
    pub per_ip_ping_pps: u32,
    /// Maximum packets per second per connected session.
    #[serde(default = "RateLimitConfig::default_per_session_pps")]
    pub per_session_pps: u32,
    /// Minimum valid packet size in bytes.
    #[serde(default = "RateLimitConfig::default_min_packet_size")]
    pub min_packet_size: usize,
    /// Maximum valid packet size in bytes (RakNet MTU).
    #[serde(default = "RateLimitConfig::default_max_packet_size")]
    pub max_packet_size: usize,
    /// Handshake failures before temporary ban.
    #[serde(default = "RateLimitConfig::default_max_handshake_failures")]
    pub max_handshake_failures: u32,
    /// Temporary ban duration in seconds.
    #[serde(default = "RateLimitConfig::default_ban_duration_secs")]
    pub ban_duration_secs: u64,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            per_ip_pps: Self::default_per_ip_pps(),
            per_ip_max_sessions: Self::default_per_ip_max_sessions(),
            global_new_sessions_pps: Self::default_global_new_sessions_pps(),
            per_ip_ping_pps: Self::default_per_ip_ping_pps(),
            per_session_pps: Self::default_per_session_pps(),
            min_packet_size: Self::default_min_packet_size(),
            max_packet_size: Self::default_max_packet_size(),
            max_handshake_failures: Self::default_max_handshake_failures(),
            ban_duration_secs: Self::default_ban_duration_secs(),
        }
    }
}

impl RateLimitConfig {
    fn default_per_ip_pps() -> u32 {
        200
    }
    fn default_per_ip_max_sessions() -> u32 {
        5
    }
    fn default_global_new_sessions_pps() -> u32 {
        50
    }
    fn default_per_ip_ping_pps() -> u32 {
        10
    }
    fn default_per_session_pps() -> u32 {
        300
    }
    fn default_min_packet_size() -> usize {
        1
    }
    fn default_max_packet_size() -> usize {
        1492
    }
    fn default_max_handshake_failures() -> u32 {
        10
    }
    fn default_ban_duration_secs() -> u64 {
        30
    }
}
