use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Instant;

/// Per-IP rate limiter using a sliding window counter approach.
/// All operations are O(1) per packet. Cleanup is amortized.
pub struct RateLimiter {
    /// Per-IP packet counters with window timestamps.
    entries: Mutex<HashMap<IpAddr, IpEntry>>,
    /// Maximum packets per second per IP.
    pps_limit: u32,
    /// Maximum concurrent sessions per IP.
    session_limit: u32,
    /// Maximum new sessions per second globally.
    global_session_pps: u32,
    /// Maximum unconnected pings per second per IP.
    ping_pps_limit: u32,
    /// Maximum packets per second per connected session.
    session_pps_limit: u32,
    /// Minimum valid packet size in bytes.
    min_packet_size: usize,
    /// Maximum valid packet size in bytes.
    max_packet_size: usize,
    /// Handshake failures before temporary ban.
    max_handshake_failures: u32,
    /// Temporary ban duration in seconds.
    ban_duration_secs: u64,

    /// Global new-session counter for current window.
    global_session_count: AtomicU64,
    /// Timestamp of current global session window.
    global_session_window: Mutex<Instant>,
}

struct IpEntry {
    /// Packet count in current window.
    packet_count: u32,
    /// Ping count in current window.
    ping_count: u32,
    /// Window start time.
    window_start: Instant,
    /// Active session count for this IP.
    session_count: u32,
    /// Consecutive handshake failures.
    handshake_failures: u32,
    /// Temporary ban expiry (if banned).
    banned_until: Option<Instant>,
}

impl IpEntry {
    fn new(now: Instant) -> Self {
        Self {
            packet_count: 0,
            ping_count: 0,
            window_start: now,
            session_count: 0,
            handshake_failures: 0,
            banned_until: None,
        }
    }

    fn reset_window_if_expired(&mut self, now: Instant) {
        if now.duration_since(self.window_start).as_secs() >= 1 {
            self.packet_count = 0;
            self.ping_count = 0;
            self.window_start = now;
        }
    }
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
            entries: Mutex::new(HashMap::new()),
            pps_limit: config.per_ip_pps,
            session_limit: config.per_ip_max_sessions,
            global_session_pps: config.global_new_sessions_pps,
            ping_pps_limit: config.per_ip_ping_pps,
            session_pps_limit: config.per_session_pps,
            min_packet_size: config.min_packet_size,
            max_packet_size: config.max_packet_size,
            max_handshake_failures: config.max_handshake_failures,
            ban_duration_secs: config.ban_duration_secs,
            global_session_count: AtomicU64::new(0),
            global_session_window: Mutex::new(Instant::now()),
        }
    }

    /// Check if a packet from this IP should be allowed.
    pub fn check_packet(&self, ip: IpAddr, packet_len: usize) -> RateLimitResult {
        if packet_len < self.min_packet_size || packet_len > self.max_packet_size {
            return RateLimitResult::InvalidPacketSize;
        }

        let now = Instant::now();
        let mut entries = self.entries.lock().unwrap();
        let entry = entries.entry(ip).or_insert_with(|| IpEntry::new(now));
        entry.reset_window_if_expired(now);

        if let Some(banned_until) = entry.banned_until {
            if now < banned_until {
                return RateLimitResult::IpBanned;
            }
            entry.banned_until = None;
            entry.handshake_failures = 0;
        }

        entry.packet_count += 1;
        if entry.packet_count > self.pps_limit {
            return RateLimitResult::IpPpsExceeded;
        }

        RateLimitResult::Allow
    }

    /// Check if an unconnected ping from this IP should be allowed.
    pub fn check_ping(&self, ip: IpAddr) -> RateLimitResult {
        let now = Instant::now();
        let mut entries = self.entries.lock().unwrap();
        let entry = entries.entry(ip).or_insert_with(|| IpEntry::new(now));
        entry.reset_window_if_expired(now);

        entry.ping_count += 1;
        if entry.ping_count > self.ping_pps_limit {
            return RateLimitResult::PingRateExceeded;
        }

        RateLimitResult::Allow
    }

    /// Check if a new session from this IP should be allowed.
    pub fn check_new_session(&self, ip: IpAddr) -> RateLimitResult {
        let now = Instant::now();

        // Check global rate
        {
            let mut window = self.global_session_window.lock().unwrap();
            if now.duration_since(*window).as_secs() >= 1 {
                self.global_session_count.store(0, Ordering::Relaxed);
                *window = now;
            }
        }
        let global_count = self.global_session_count.fetch_add(1, Ordering::Relaxed);
        if global_count >= self.global_session_pps as u64 {
            return RateLimitResult::GlobalSessionRateExceeded;
        }

        // Check per-IP limit
        let mut entries = self.entries.lock().unwrap();
        let entry = entries.entry(ip).or_insert_with(|| IpEntry::new(now));

        if let Some(banned_until) = entry.banned_until {
            if now < banned_until {
                return RateLimitResult::IpBanned;
            }
            entry.banned_until = None;
            entry.handshake_failures = 0;
        }

        if entry.session_count >= self.session_limit {
            return RateLimitResult::SessionLimitExceeded;
        }

        entry.session_count += 1;
        RateLimitResult::Allow
    }

    /// Notify that a session for this IP was closed.
    pub fn session_closed(&self, ip: IpAddr) {
        let mut entries = self.entries.lock().unwrap();
        if let Some(entry) = entries.get_mut(&ip) {
            entry.session_count = entry.session_count.saturating_sub(1);
        }
    }

    /// Record a handshake failure for this IP. May trigger a temp ban.
    pub fn record_handshake_failure(&self, ip: IpAddr) {
        let now = Instant::now();
        let mut entries = self.entries.lock().unwrap();
        let entry = entries.entry(ip).or_insert_with(|| IpEntry::new(now));
        entry.handshake_failures += 1;
        if entry.handshake_failures >= self.max_handshake_failures {
            let ban_until = now + std::time::Duration::from_secs(self.ban_duration_secs);
            entry.banned_until = Some(ban_until);
            log::warn!(
                "Temporarily banned {} for {} repeated handshake failures",
                ip,
                entry.handshake_failures
            );
        }
    }

    /// Record a successful handshake, resetting failure count.
    pub fn record_handshake_success(&self, ip: IpAddr) {
        let mut entries = self.entries.lock().unwrap();
        if let Some(entry) = entries.get_mut(&ip) {
            entry.handshake_failures = 0;
        }
    }

    /// Returns the per-session pps limit.
    pub fn session_pps_limit(&self) -> u32 {
        self.session_pps_limit
    }

    /// Periodic cleanup of stale entries (call every ~30s from a background task).
    pub fn cleanup_stale_entries(&self) {
        let now = Instant::now();
        let mut entries = self.entries.lock().unwrap();
        entries.retain(|_, entry| {
            let age = now.duration_since(entry.window_start).as_secs();
            // Keep entries that have active sessions or are recently active or banned
            entry.session_count > 0
                || age < 60
                || entry.banned_until.is_some_and(|t| now < t)
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

use serde::{Deserialize, Serialize};

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
