As of **March 2, 2026**, I ran Exa-led research across official docs/repos and distilled this.

**1) Core L4 Proxy Design Paradigms (what modern high-efficiency systems converge on)**
1. **Event-driven, non-blocking I/O**  
   HAProxy, NGINX, and Envoy all center on this model.
2. **Thread/core-aware parallelism**  
   Common patterns: one worker per CPU/hardware thread, RSS/reuseport, minimizing cross-thread locking.
3. **Connection-affinity + deterministic hashing**  
   Maglev/consistent hash, per-flow stickiness, backend-failure resilience.
4. **Zero-copy and copy-minimization**  
   `sendfile`, kernel splice paths, AF_XDP zero-copy modes, io_uring zerocopy send APIs.
5. **Kernel-bypass user-space datapaths (DPDK class)**  
   Poll-mode, no interrupt path, batch TX/RX, per-core lockless data structures.
6. **Kernel-resident packet steering (eBPF/XDP class)**  
   Packet handling before full kernel stack path; lowest-latency L4 forwarding for stateless/near-stateless paths.
7. **Hot reload / graceful restart as a first-class requirement**  
   Envoy, Pingora, and HAProxy ecosystems strongly emphasize this for SLO-safe upgrades.

---

**2) Top Open-Source L4 Projects/Frameworks (lightweight, stable, performant)**
1. **HAProxy** (user-space, classic high-performance L4/L7)
2. **NGINX stream** (TCP/UDP proxying module in mainstream NGINX)
3. **Envoy** (extensible L4/L7 proxy with TCP/UDP filters)
4. **Cloudflare Pingora** (Rust async framework for high-scale proxies)
5. **Meta Katran** (eBPF/XDP L4 forwarder)
6. **Cilium** (eBPF/XDP kube-proxy replacement + L4 load-balancing path)
7. **DPVS** (DPDK-based L4 load balancer)
8. **Sōzu** (Rust TCP/HTTP reverse proxy, runtime-reconfigurable)

---

**3) Standards Baseline: HAProxy + NGINX stream**
1. **HAProxy**
   - C, multi-threaded, event-driven, non-blocking daemon.
   - Single process, multiple worker threads, near-linear scale intent with low inter-thread dependency.
   - Strong release/LTS discipline visible on project site.
2. **NGINX stream**
   - Raw TCP/UDP proxying (`ngx_stream_proxy_module`) with upstream balancing and phase-based processing.
   - Master/worker architecture, event APIs (`epoll`/`kqueue`), sendfile/copy minimization, low memory footprint.

---

**4/5) Project-by-project analysis (a-d)**

| Project | (a) Language + execution model | (b) Architecture choices | (c) Performance / efficiency evidence | (d) Reliability + enterprise adoption |
|---|---|---|---|---|
| **HAProxy** | C; multi-threaded event loop | Single process, many worker threads; per-connection thread affinity | Docs explicitly target near-linear scalability with minimized inter-thread dependencies | Long LTS/release cadence; widely packaged/distributed |
| **NGINX stream** | C; master + worker event loops | Stream phases, TCP/UDP upstream proxying, reuseport, sendfile support | Official docs highlight low resource use; 10k idle keep-alives ~2.5MB memory | NGINX reports “world’s most popular web server”; stream is mature and widely deployed |
| **Envoy (L4 features)** | C++; primary + worker threads, non-blocking | TCP proxy filter + UDP proxy filter, dynamic routing/filter chains, hot restart | No canonical “one-number” pps benchmark in official docs; architecture optimized for parallel non-blocking path | CNCF graduated; broad enterprise adoption called out by CNCF/Envoy |
| **Pingora** | Rust async multithreaded (Tokio ecosystem) | Shared connection pools across threads, programmable filters/callbacks, graceful restart | Cloudflare reports: over 1T req/day (2022), ~70% CPU and ~67% memory reduction vs prior stack, better TTFB, massive connection reuse gains | Production at Cloudflare scale; OSS since 2024; public CVE response/patch timeline in 2025 |
| **Katran** | C++ control + eBPF/XDP dataplane | XDP fast path, lockless per-CPU maps, Maglev, DSR/IPIP encapsulation | Meta + repo describe linear scaling with RX queues and elimination of idle busy loops | Used in Meta PoPs; mature OSS project with active community |
| **Cilium (LB path)** | Go control plane + eBPF/XDP datapath | kube-proxy replacement, socket LB, optional XDP acceleration, L3/L4 focus with broader stack | Scalability report shows tested operation at 1000-worker-node / 50k-pod scale with documented CPU/memory behavior | CNCF graduated; CNCF cites well over 100 adopting orgs and large production footprints |
| **DPVS** | C + DPDK user-space dataplane | Kernel bypass, poll mode, per-CPU lockless, batching, zero-copy, multiple L4 forwarding modes | Core project emphasizes techniques; derivative Intel HDSLB (DPVS-based) reports rough >8 Mpps/core and 100M session class numbers | Strong OSS footprint in DPDK/L4 circles; adoption concentrated in performance-focused infra teams |
| **Sōzu (Rust alt)** | Rust; share-nothing worker model with epoll | Single-thread workers, SO_REUSEPORT, runtime config diffs via control socket | Public architecture emphasizes low synchronization overhead; limited standardized public benchmark corpus | Used by cloud/PaaS operators per project site; smaller ecosystem than above leaders |

---

**6) User-space proxies vs kernel-level eBPF/XDP forwarders (lightweight/raw profile)**

| Dimension | User-space proxy (HAProxy/Envoy/Pingora/NGINX stream) | Kernel-level forwarder (Katran/Cilium XDP path) |
|---|---|---|
| Latency path | Higher (kernel↔user crossings, socket layer) | Lower (early packet hook, less traversal) |
| Feature depth | Much richer (TLS termination, L7-aware policies, extensibility) | Narrower (best for L3/L4 forwarding/load distribution) |
| Operational complexity | Familiar tooling, easier debugging | Harder (kernel verifier constraints, NIC/kernel dependencies) |
| State handling | Easier for complex per-connection/per-request logic | Prefer stateless/min-state fast paths |
| Failure domain | Process crash isolation | Kernel-path bugs can be higher-impact |
| Hardware efficiency | Good to excellent | Best raw pps/CPU for suitable workloads |

---

**7) Recommendations for building a modern L4 reverse proxy**

1. **Use a hybrid architecture** (best practical pattern).  
   Inference from sources: fastest systems split responsibilities.  
   - XDP/eBPF front path for stateless L4 steering/DSR  
   - User-space proxy tier for richer policy, TLS, observability, and slow-path handling
2. **Reference projects to study first (highest value):**  
   **Katran**, **Cilium**, **HAProxy**, **Envoy**, **Pingora**.
3. **If you need pure raw L4 and minimalism:**  
   Start from **Katran-style** (or Cilium LB internals in K8s contexts).
4. **If you need “raw + operable + rich controls” in one binary path:**  
   Start from **HAProxy** (maturity) or **Pingora** (Rust/memory-safety + performance).
5. **For dedicated appliance-style throughput engineering:**  
   Study **DPVS + DPDK** patterns.
6. **Benchmarking guidance:**  
   Do not compare vendor numbers directly; run uniform p50/p95 latency, pps/cps, and CPU/core efficiency tests on your own NIC/kernel/workload mix.

---

**Primary sources**
- https://docs.haproxy.org/3.0/management.html  
- https://www.haproxy.org/  
- https://www.haproxy.org/they-use-it.html  
- https://nginx.org/en/  
- https://nginx.org/en/docs/stream/ngx_stream_core_module.html  
- https://nginx.org/en/docs/stream/ngx_stream_proxy_module.html  
- https://nginx.org/en/docs/stream/stream_processing.html  
- https://www.envoyproxy.io/docs/envoy/latest/intro/arch_overview/intro/threading_model  
- https://www.envoyproxy.io/docs/envoy/latest/configuration/listeners/network_filters/tcp_proxy_filter  
- https://www.envoyproxy.io/docs/envoy/latest/configuration/listeners/udp_filters/udp_proxy  
- https://www.envoyproxy.io/docs/envoy/latest/intro/arch_overview/operations/hot_restart  
- https://www.envoyproxy.io/  
- https://www.cncf.io/announcements/2018/11/28/cncf-announces-envoy-graduation/  
- https://www.cncf.io/projects/envoy/  
- https://blog.cloudflare.com/how-we-built-pingora-the-proxy-that-connects-cloudflare-to-the-internet/  
- https://blog.cloudflare.com/pingora-open-source/  
- https://blog.cloudflare.com/pingora-saving-compute-1-percent-at-a-time/  
- https://blog.cloudflare.com/resolving-a-request-smuggling-vulnerability-in-pingora/  
- https://github.com/cloudflare/pingora  
- https://engineering.fb.com/2018/05/22/open-source/open-sourcing-katran-a-scalable-network-load-balancer/  
- https://github.com/facebookincubator/katran  
- https://docs.cilium.io/en/stable/network/kubernetes/kubeproxy-free.html  
- https://docs.cilium.io/en/stable/operations/performance/scalability/report/  
- https://docs.cilium.io/en/stable/reference-guides/bpf/  
- https://www.cncf.io/announcements/2023/10/11/cloud-native-computing-foundation-announces-cilium-graduation/  
- https://www.cncf.io/projects/cilium/  
- https://github.com/iqiyi/dpvs  
- https://doc.dpdk.org/guides-24.03/prog_guide/poll_mode_drv.html  
- https://doc.dpdk.org/guides-25.11/nics/af_xdp.html  
- https://man7.org/linux/man-pages/man2/sendfile.2.html  
- https://man7.org/linux/man-pages/man3/io_uring_prep_send_zc.3.html  
- https://www.sozu.io/  
- https://github.com/yyyar/gobetween