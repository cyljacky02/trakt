use std::sync::atomic::{AtomicUsize, Ordering};

/// Lightweight atomic counters for proxy observability.
#[derive(Debug, Default)]
pub struct Metrics {
    /// Total packets received from downstream clients.
    pub packets_received: AtomicUsize,
    /// Packets relayed on the fast path (connected sessions).
    pub packets_relayed: AtomicUsize,
    /// Packets dropped due to backpressure.
    pub packets_dropped: AtomicUsize,
    /// Currently active sessions.
    pub active_sessions: AtomicUsize,
    /// Sessions closed due to timeout.
    pub timeout_disconnects: AtomicUsize,
}

impl Metrics {
    /// Returns a snapshot of all counters for display.
    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            packets_received: self.packets_received.load(Ordering::Relaxed),
            packets_relayed: self.packets_relayed.load(Ordering::Relaxed),
            packets_dropped: self.packets_dropped.load(Ordering::Relaxed),
            active_sessions: self.active_sessions.load(Ordering::Relaxed),
            timeout_disconnects: self.timeout_disconnects.load(Ordering::Relaxed),
        }
    }
}

/// Point-in-time snapshot of metrics values.
#[derive(Debug, Clone)]
pub struct MetricsSnapshot {
    pub packets_received: usize,
    pub packets_relayed: usize,
    pub packets_dropped: usize,
    pub active_sessions: usize,
    pub timeout_disconnects: usize,
}

impl std::fmt::Display for MetricsSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "sessions={} recv={} relayed={} dropped={} timeouts={}",
            self.active_sessions,
            self.packets_received,
            self.packets_relayed,
            self.packets_dropped,
            self.timeout_disconnects,
        )
    }
}
