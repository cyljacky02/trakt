As of **March 2, 2026 (UTC+8)**, here is a source-grounded research summary.

## 1) Open-source Rust UDP reverse proxy projects

| Project | UDP architecture (session / routing / state+reliability) | Perf + memory evidence | Stability + maintainer activity |
|---|---|---|---|
| **rathole** | Uses control + data channels; UDP datagrams are framed as `UdpTraffic`; client keeps `UdpPortMap` keyed by visitor `SocketAddr`; per-visitor UDP forwarder is cleaned on timeout; app-layer heartbeat (`heartbeat_interval` / `heartbeat_timeout`). | Has benchmark doc (dated **2021-12-28**) vs frp, incl. UDP bitrate, HTTP latency, and memory graph; README claims low memory and ~500KiB minimal binary. | Mature and active. Last push: **2026-02-08** (from GitHub API snapshot). |
| **Quilkin** | Explicit UDP “Session” model as 4-tuple `(client ip, client port, server ip, server port)`; session auto-created post-filter-chain and expires after 60s idle; routing is filter-chain driven (TokenRouter, LB, firewall, etc.). | Strong metrics surface (session histograms, packet processing duration, filter timings), but no single canonical req/s benchmark in repo docs. | Active and production-used but marked beta. Last push: **2026-02-27**. |
| **rust-rpxy-l4** | UDP pseudo-connections keyed by client socket; protocol-aware routing (QUIC/WireGuard), optional SNI/ALPN for QUIC/TLS mux, configurable idle lifetimes and UDP LB methods. | No published benchmark numbers in repo docs. | Explicitly WIP/early-stage and “not recommended for production”. Last push: **2026-02-27**. |
| **udppp** | HashMap of source addr -> upstream socket + timestamp; creates per-source ephemeral upstream socket; forwards packets and removes mappings on timeout; optional Proxy Protocol V2 and mmproxy mode. | Includes dated benchmark (**2022-02-28**) with small-loop UDP test against nginx/go-proxy; no modern large-scale perf suite. | Lightweight but low recent engineering velocity. Last push: **2022-04-20**. |
| **trakt** | RakNet-aware Bedrock proxy; tracks clients in HashMap with connection stages; server selection via RR/least-connected; MOTD and health checks; snapshot-based restart recovery for active sessions. | README explicitly warns reliability/performance claims may not yet be true; no benchmark suite. | Useful RakNet design reference but less active. Last push: **2024-08-22**. |
| **ccproxy** | RakNet listener accepts client sessions, opens upstream RakNet socket per client, runs bidirectional relays (`c2s`/`s2c`); query token/state handling and periodic MOTD/query syncing. | “High-performance” claim, but no benchmark numbers in repo docs. | Newer and active in 2026. Last push: **2026-02-24**. |
| **taxy** | Supports UDP/TCP/HTTP/TLS/WebSocket + live reconfig, but public docs are high-level; UDP session internals are not well documented in README. | No public benchmark/memory study in main docs. | Early-development warning; activity cooled after 2025 release line. Last push: **2025-04-06**. |

## 2) Rust projects that specifically support/reference RakNet

- **Direct RakNet game-server proxies**
1. **trakt** (explicitly RakNet-aware Bedrock reverse proxy/load balancer).
2. **ccproxy** (Bedrock proxy using `rust-raknet` dependency and RakNet socket flow).

- **RakNet protocol libraries (not full reverse proxies, but core references)**
1. **b23r0/rust-raknet**
2. **NetrexMC/RakNet**

## 3) High-performance Rust async frameworks/crates for UDP proxies

| Framework/crate | Architecture fit for UDP proxying (a/b/c) | Perf/stability notes |
|---|---|---|
| **Tokio** | Work-stealing scheduler + reactor (`epoll/kqueue/IOCP`) + async UDP sockets; session/state is app-owned. | Best ecosystem maturity and ops stability; LTS policy and very active maintenance. |
| **Mio** | Low-level readiness/poller; app fully owns UDP state machine/session map/routing. | Minimal overhead, no runtime allocations in core path; best when you need full control. |
| **Monoio** | Thread-per-core runtime (`io_uring`/`epoll`/`kqueue`); avoids cross-thread task migration, good for sharded UDP maps. | Repo benchmark doc (2021) claims stronger scaling than Tokio/Glommio in tested setup; active project. |
| **Glommio** | Cooperative thread-per-core on `io_uring`, no helper threads; great for CPU/cache locality per shard. | Good architecture, but release cadence less frequent than Tokio/Monoio; verify fit per workload. |
| **tokio-uring** | Tokio-compatible runtime with `io_uring` driver; great for Linux-specific low-overhead I/O; single-runtime-thread model (scale via multiple runtimes/threads). | Useful, but younger and less turnkey than Tokio for broad production workloads. |
| **Quinn** | QUIC over UDP with explicit endpoint/connection/session model; built-in congestion control, retransmission, flow control. | Strong choice when you need reliable UDP transport semantics. |
| **s2n-quic** | QUIC with provider-based architecture, CUBIC/pacing/GSO/PMTU; deep test/sim/compliance pipeline. | Strong engineering rigor; excellent for high-assurance reliable-UDP systems. |

## 4) Synthesis: most valuable references + modern/lightweight patterns

### Most valuable references by use case
1. **General-purpose UDP reverse tunneling:** `rathole`
2. **Game-server UDP policy routing / filter pipeline:** `Quilkin`
3. **RakNet/Bedrock proxy reference:** `ccproxy` (more recent activity) + `trakt` (nice recovery/load-balancer ideas)
4. **Protocol-multiplexed L4 edge (QUIC/WireGuard/TLS):** `rust-rpxy-l4`
5. **Minimal/simple UDP forwarder baseline:** `udppp`

### Design patterns that best match “modern, performant, lightweight”
1. **Control-plane / data-plane split** (rathole-style control channel + on-demand data channels).
2. **UDP pseudo-session map + idle GC** keyed by client socket tuple.
3. **Protocol-aware packet classification before forwarding** (filter chain or handshake probing).
4. **Session-level observability** (active sessions, duration histograms, drop reasons, filter timings).
5. **Hot-reload config + fail-safe defaults** (no restart required for route/service changes).
6. **Reliable UDP only when needed** via QUIC libs (`quinn` / `s2n-quic`) instead of reinventing reliability.
7. **Runtime choice by workload shape**: Tokio for broad production portability; thread-per-core runtimes for extreme Linux throughput and strict sharding.

## Sources
- rathole: https://github.com/rathole-org/rathole  
- rathole benchmark doc: https://raw.githubusercontent.com/rathole-org/rathole/main/docs/benchmark.md  
- rathole client internals: https://raw.githubusercontent.com/rathole-org/rathole/main/src/client.rs  
- rathole server internals: https://raw.githubusercontent.com/rathole-org/rathole/main/src/server.rs  
- Quilkin: https://github.com/googleforgames/quilkin  
- Quilkin UDP/session docs: https://raw.githubusercontent.com/googleforgames/quilkin/main/docs/src/services/udp.md  
- Quilkin metrics docs: https://raw.githubusercontent.com/googleforgames/quilkin/main/docs/src/deployment/metrics.md  
- rust-rpxy-l4: https://github.com/junkurihara/rust-rpxy-l4  
- udppp: https://github.com/b23r0/udppp  
- udppp main: https://raw.githubusercontent.com/b23r0/udppp/main/src/main.rs  
- udppp mmproxy: https://raw.githubusercontent.com/b23r0/udppp/main/src/mmproxy.rs  
- trakt: https://github.com/Unoqwy/trakt  
- ccproxy: https://github.com/chungchandev/ccproxy  
- ccproxy run path: https://raw.githubusercontent.com/chungchandev/ccproxy/main/src/cli/run.rs  
- ccproxy Cargo (RakNet dependency): https://raw.githubusercontent.com/chungchandev/ccproxy/main/Cargo.toml  
- tokio: https://github.com/tokio-rs/tokio  
- mio: https://github.com/tokio-rs/mio  
- monoio: https://github.com/bytedance/monoio  
- monoio benchmark doc: https://raw.githubusercontent.com/bytedance/monoio/master/docs/en/benchmark.md  
- glommio: https://github.com/DataDog/glommio  
- tokio-uring: https://github.com/tokio-rs/tokio-uring  
- quinn: https://github.com/quinn-rs/quinn  
- s2n-quic: https://github.com/aws/s2n-quic  
- s2n-quic CI/perf pipeline: https://raw.githubusercontent.com/aws/s2n-quic/main/docs/dev-guide/ci.md