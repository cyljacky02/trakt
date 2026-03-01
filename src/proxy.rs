use rand::Rng;
use std::fmt;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;
use std::{collections::HashMap, net::SocketAddr, sync::Arc};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::config::ConfigProvider;
use crate::health::HealthController;
use crate::load_balancer::{BackendServer, LoadBalancer};
use crate::metrics::Metrics;
use crate::motd::MOTDReflector;
use crate::raknet;
use crate::raknet::{
    datatypes::ReadBuf,
    frame::Frame,
    message::{Message, MessageUnconnectedPing, MessageUnconnectedPong, RaknetMessage},
};
use bytes::{Buf, Bytes};
use tokio::{
    net::{ToSocketAddrs, UdpSocket},
    sync::Semaphore,
};

use ppp::v2 as haproxy;

/// Maximum concurrent slow-path (new session / offline message) handlers.
const MAX_SLOW_PATH_CONCURRENT: usize = 256;

/// Connection stage constants.
const STAGE_HANDSHAKE: u8 = 0;
const STAGE_CONNECTED: u8 = 1;
const STAGE_CLOSED: u8 = 2;

/// Raknet proxy server that manages connections and uses
/// the load balancer to pick the server for new connections.
///
/// It will forward all the traffic, except offline (no initialized Raknet connection
/// with the server) MOTD requests.
pub struct RaknetProxy {
    /// UDP socket for Player <-> Proxy traffic.
    in_udp_sock: Arc<UdpSocket>,
    /// Cached port from `in_udp_sock`.
    in_bound_port: u16,

    /// Random ID consistent during the lifetime of the proxy
    /// representing the server.
    server_uuid: i64,
    /// All current clients of the proxy.
    clients: Arc<std::sync::RwLock<HashMap<SocketAddr, Arc<RaknetClient>>>>,

    /// Config provider.
    config_provider: Arc<ConfigProvider>,
    /// MOTD reflector.
    motd_reflector: Arc<MOTDReflector>,
    /// Load balancer.
    load_balancer: LoadBalancer,
    /// Health controller.
    health_controller: Arc<HealthController>,
    /// Cancellation token for stopping background tasks.
    cancel_token: CancellationToken,

    /// Metrics counters.
    metrics: Arc<Metrics>,

    /// Backpressure semaphore for slow-path spawns.
    slow_path_semaphore: Arc<Semaphore>,
}

/// A client to the proxy.
///
/// Since UDP is a connectionless protocol, any mention of "connection"
/// is in fact an emulated connection, aka. session.
struct RaknetClient {
    /// Remote player client address.
    addr: SocketAddr,
    /// Backend server.
    server: Arc<BackendServer>,
    /// UDP socket for Player <-> Proxy traffic.
    proxy_udp_sock: Arc<UdpSocket>,
    /// UDP socket for Proxy <-> Server traffic (connected to backend).
    udp_sock: UdpSocket,
    /// Cached local socket address of `udp_sock`.
    udp_sock_addr: SocketAddr,
    /// Connection stage.
    stage: AtomicU8,

    /// Close notifier.
    close_tx: mpsc::Sender<DisconnectCause>,
    /// Semaphore used to wait for guaranteed close state.
    close_lock: Semaphore,

    /// Cached debug prefix string (player→server direction).
    prefix_p2s: String,
    /// Cached debug prefix string (server→player direction).
    prefix_s2p: String,
}

/// Result of spying into a datagram packet.
enum SpyDatagramResult {
    /// Nothing that we need to know about, ignore.
    Ignore,
    /// The datagram contains a [`RaknetMessage::DisconnectNotification`].
    Disconnect,
}

/// Data flow direction.
#[derive(Debug, Clone, Copy)]
enum Direction {
    /// Player <-> Server
    PlayerToServer,
    /// Server <-> Player
    ServerToPlayer,
}

impl RaknetClient {
    #[inline]
    fn stage(&self) -> u8 {
        self.stage.load(Ordering::Acquire)
    }

    /// Returns true if the client is in the Connected stage.
    #[inline]
    fn is_connected(&self) -> bool {
        self.stage() == STAGE_CONNECTED
    }
}

/// Why a player disconnected from a server.
#[derive(Debug, Clone, Copy)]
enum DisconnectCause {
    /// Found disconnect notification from the client.
    Client,
    /// Found disconnect notification from the server.
    Server,
    /// Connection timed out.
    Timeout,
    /// An unexpected error occurred.
    Error,
    /// Unknown cause.
    Unknown,
}

/// Overview of the load of a [`RaknetProxy`].
#[derive(Debug, Clone)]
pub struct LoadOverview {
    /// Number of active clients.
    pub client_count: usize,
    /// Out of the active clients, how many are actually connected.
    pub connected_count: usize,
    /// Breakdown of the load per server.
    pub per_server: HashMap<SocketAddr, usize>,
}

impl RaknetProxy {
    /// Attempts to bind a proxy server to a UDP socket.
    ///
    /// ## Arguments
    ///
    /// * `in_addr` - Address to bind to for Player <-> Proxy traffic
    /// * `config_provider` - Config provider
    pub async fn bind<A: ToSocketAddrs>(
        in_addr: A,
        config_provider: Arc<ConfigProvider>,
    ) -> std::io::Result<Arc<Self>> {
        let in_udp_sock = UdpSocket::bind(in_addr).await?;
        let in_bound_port = in_udp_sock.local_addr()?.port();
        let server_uuid = rand::thread_rng().r#gen();
        let motd_reflector = Arc::new(MOTDReflector::new(config_provider.clone()));
        let health_controller = Arc::new(HealthController::new(config_provider.clone()));
        let load_balancer =
            LoadBalancer::init(config_provider.clone(), health_controller.clone()).await;
        let cancel_token = CancellationToken::new();
        let metrics = Arc::new(Metrics::default());
        Ok(Arc::new(Self {
            in_udp_sock: Arc::new(in_udp_sock),
            in_bound_port,
            server_uuid,
            config_provider,
            clients: Arc::new(std::sync::RwLock::new(HashMap::new())),
            motd_reflector,
            load_balancer,
            health_controller,
            cancel_token,
            metrics,
            slow_path_semaphore: Arc::new(Semaphore::new(MAX_SLOW_PATH_CONCURRENT)),
        }))
    }

    /// Reloads configuration and restarts background tasks.
    pub async fn reload_config(&self) {
        self.load_balancer.reload_config().await;
        // Background tasks re-read config on next tick automatically
    }

    /// Obtains a load overview.
    pub fn load_overview(&self) -> LoadOverview {
        let clients = self.clients.read().unwrap();
        let mut per_server = HashMap::new();
        let mut client_count = 0;
        let mut connected_count = 0;
        for (_, client) in clients.iter() {
            let server_load = per_server.entry(client.server.addr).or_default();
            *server_load += 1;
            client_count += 1;
            if client.is_connected() {
                connected_count += 1;
            }
        }
        LoadOverview {
            client_count,
            connected_count,
            per_server,
        }
    }

    /// Starts background tasks (health checks + MOTD refresh).
    pub fn start_background_tasks(self: &Arc<Self>) {
        // Health check loop
        tokio::spawn({
            let config_provider = self.config_provider.clone();
            let health_controller = self.health_controller.clone();
            let token = self.cancel_token.clone();
            async move {
                let rate = {
                    let config = config_provider.read().await;
                    Duration::from_secs(u64::max(config.backend.health_check_rate, 1))
                };
                let mut interval = tokio::time::interval(rate);
                loop {
                    tokio::select! {
                        _ = token.cancelled() => return,
                        _ = interval.tick() => health_controller.execute().await,
                    }
                }
            }
        });
        // MOTD refresh loop
        tokio::spawn({
            let config_provider = self.config_provider.clone();
            let motd_reflector = self.motd_reflector.clone();
            let token = self.cancel_token.clone();
            async move {
                let rate = {
                    let config = config_provider.read().await;
                    Duration::from_secs(u64::max(config.backend.motd_refresh_rate, 1))
                };
                let mut interval = tokio::time::interval(rate);
                loop {
                    tokio::select! {
                        _ = token.cancelled() => return,
                        _ = interval.tick() => motd_reflector.execute().await,
                    }
                }
            }
        });
    }

    /// Runs the proxy server with inline fast-path and bounded slow-path.
    ///
    /// If stopped gracefully it will return `Ok(())`, otherwise it will return an error.
    pub async fn run(self: Arc<Self>) -> anyhow::Result<()> {
        self.start_background_tasks();
        log::info!(
            "Starting Raknet proxy server on {}",
            self.in_udp_sock.local_addr()?
        );

        let udp_sock = self.in_udp_sock.clone();
        let mut buf = [0u8; 1492];
        loop {
            let (len, addr) = udp_sock.recv_from(&mut buf).await?;
            if len == 0 {
                continue;
            }
            self.metrics
                .packets_received
                .fetch_add(1, Ordering::Relaxed);

            // Fast path: relay for existing connected sessions
            let fast_client = {
                let clients = self.clients.read().unwrap();
                clients.get(&addr).filter(|c| c.is_connected()).cloned()
            };
            if let Some(client) = fast_client {
                if let Err(err) = client.udp_sock.send(&buf[..len]).await {
                    log::debug!("{} Unable to forward data: {:?}", client.prefix_p2s, err);
                }
                // Spy for disconnect in-line (cheap)
                if buf[0] & 0x80 != 0 {
                    let data = Bytes::copy_from_slice(&buf[..len]);
                    if matches!(
                        client.spy_datagram(Direction::PlayerToServer, data),
                        Ok(SpyDatagramResult::Disconnect)
                    ) {
                        log::debug!(
                            "{} Found disconnect notification in datagram",
                            client.prefix_p2s,
                        );
                        let _ = client.close_tx.send(DisconnectCause::Client).await;
                    }
                }
                self.metrics.packets_relayed.fetch_add(1, Ordering::Relaxed);
                continue;
            }

            // Slow path: new session, offline message, or handshake — bounded spawn
            let data = Bytes::copy_from_slice(&buf[..len]);
            let permit = match self.slow_path_semaphore.clone().try_acquire_owned() {
                Ok(permit) => permit,
                Err(_) => {
                    log::warn!("[{}] Slow-path backpressure: dropping packet", addr);
                    self.metrics.packets_dropped.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
            };
            tokio::spawn({
                let proxy = self.clone();
                async move {
                    if let Err(err) = proxy.handle_recv_slow(addr, data).await {
                        log::debug!(
                            "[{}] Unable to handle player -> server UDP datagram message: {:?}",
                            addr,
                            err
                        );
                    }
                    drop(permit);
                }
            });
        }
    }

    /// Performs a cleanup after the proxy stopped.
    pub async fn cleanup(&self) {
        self.cancel_token.cancel();
    }

    /// Slow-path handler for packets that need session creation or offline processing.
    ///
    /// ## Arguments
    ///
    /// * `addr` - Remote player client address
    /// * `data` - Raw received data
    async fn handle_recv_slow(&self, addr: SocketAddr, data: Bytes) -> anyhow::Result<()> {
        let message_type = RaknetMessage::from_u8(data[0]);
        let client = {
            let clients = self.clients.read().unwrap();
            clients.get(&addr).cloned()
        };
        match (message_type, client) {
            (
                Some(
                    RaknetMessage::UnconnectedPing | RaknetMessage::UnconnectedPingOpenConnections,
                ),
                _,
            ) => {
                let mut buf = ReadBuf::new(data);
                let _ = buf.read_u8()?;
                self.handle_unconnected_ping(addr, buf).await?;
            }
            (_, Some(client)) if client.is_connected() => {
                // Connected client that wasn't caught in fast path (race) — just relay
                if let Err(err) = client.handle_incoming_player(data).await {
                    log::debug!(
                        "{} Unable to handle UDP datagram message: {:?}",
                        client.prefix_p2s,
                        err
                    );
                }
            }
            (Some(message_type), mut client) => {
                log::trace!("[{}] Received offline message {:?}", addr, message_type);
                if client.is_none() || message_type.eq(&RaknetMessage::OpenConnectionRequest1) {
                    if let Some(client) = client {
                        let _ = client.close_tx.send(DisconnectCause::Unknown).await;
                        let _ = client.close_lock.acquire().await;
                    }
                    let new_client = self.new_client(addr, STAGE_HANDSHAKE, None).await?;
                    client = Some(new_client);
                }
                client.unwrap().forward_to_server(&data).await;
            }
            _ => {}
        }
        Ok(())
    }

    /// Creates and inserts a new client.
    /// The caller is responsible for ensuring it would not overwrite an existing client,
    /// otherwise an error will be returned and the client won't be created.
    ///
    /// ## Arguments
    ///
    /// * `addr` - Remote player client address
    /// * `stage` - Connection stage constant (`STAGE_HANDSHAKE` for new ones)
    /// * `server` - Specific backend server. If [`None`], one will be picked
    ///   from the load balancer.
    async fn new_client(
        &self,
        addr: SocketAddr,
        stage: u8,
        server: Option<Arc<BackendServer>>,
    ) -> anyhow::Result<Arc<RaknetClient>> {
        let (proxy_bind, proxy_protocol, handshake_timeout, backend_silence_timeout) = {
            let config = self.config_provider.read().await;
            (
                config.proxy_bind.clone(),
                config.proxy_protocol.unwrap_or(true),
                Duration::from_secs(config.timeouts.handshake),
                Duration::from_secs(config.timeouts.backend_silence),
            )
        };

        let server = match server {
            Some(server) => server,
            None => match self.load_balancer.next().await {
                Some(server) => {
                    log::debug!("[{}] Picked server {}", addr, server.addr);
                    server
                }
                None => return Err(anyhow::anyhow!("No server available to proxy this player")),
            },
        };

        let sock = UdpSocket::bind(&proxy_bind).await?;
        sock.connect(server.addr).await?;
        let udp_sock_addr = sock.local_addr()?;

        let (tx, rx) = mpsc::channel(1);
        let prefix_p2s = format!(
            "[player: {} -> server {} ({})]",
            addr, server.addr, udp_sock_addr
        );
        let prefix_s2p = format!(
            "[server: {} ({}) -> player {}]",
            server.addr, udp_sock_addr, addr
        );
        let client = Arc::new(RaknetClient {
            addr,
            server,
            proxy_udp_sock: self.in_udp_sock.clone(),
            udp_sock_addr,
            udp_sock: sock,
            stage: AtomicU8::new(stage),
            close_tx: tx,
            close_lock: Semaphore::new(0),
            prefix_p2s,
            prefix_s2p,
        });

        // Scope the write lock so it's dropped before any .await
        let total = {
            let mut clients = self.clients.write().unwrap();
            if clients.contains_key(&addr) {
                return Err(anyhow::anyhow!(
                    "Failed to maintain state for client {}",
                    addr
                ));
            }
            clients.insert(addr, client.clone());
            clients.len()
        };

        self.metrics.active_sessions.fetch_add(1, Ordering::Relaxed);
        tokio::spawn({
            let client = client.clone();
            let clients = self.clients.clone();
            let metrics = self.metrics.clone();
            async move {
                client.server.load.fetch_add(1, Ordering::Release);
                let loop_result = client
                    .run_event_loop(rx, handshake_timeout, backend_silence_timeout)
                    .await;
                let client_count = {
                    let mut clients = clients.write().unwrap();
                    clients.remove(&client.addr);
                    clients.len()
                };
                let was_connected =
                    client.stage.swap(STAGE_CLOSED, Ordering::AcqRel) == STAGE_CONNECTED;
                client.close_lock.add_permits(1);
                client.server.load.fetch_sub(1, Ordering::Release);
                metrics.active_sessions.fetch_sub(1, Ordering::Relaxed);
                let cause = match loop_result {
                    Ok(cause) => {
                        log::debug!(
                            "Connection closed: {} | {} total",
                            client.addr,
                            client_count,
                        );
                        cause
                    }
                    Err(err) => {
                        log::debug!(
                            "Connection closed unexpectedly for {}: {} | {} total",
                            client.addr,
                            err,
                            client_count
                        );
                        DisconnectCause::Error
                    }
                };
                if matches!(cause, DisconnectCause::Timeout) {
                    metrics.timeout_disconnects.fetch_add(1, Ordering::Relaxed);
                }
                if was_connected {
                    log::info!(
                        "Player {} has disconnected from {} ({})",
                        client.addr,
                        client.server.addr,
                        cause,
                    )
                }
            }
        });
        log::debug!(
            "Client initialized: {} <-> {} ({}) | {} total",
            client.addr,
            client.server.addr,
            client.udp_sock_addr,
            total
        );
        if proxy_protocol {
            client.send_haproxy_info().await?;
        }
        Ok(client)
    }

    /// Handles a ping request from an offline message (aka. unconnected ping request).
    ///
    /// ## Arguments
    ///
    /// * `addr` - Remote player client address
    /// * `buf` - Buffer to read the request from
    async fn handle_unconnected_ping(
        &self,
        addr: SocketAddr,
        mut buf: ReadBuf,
    ) -> anyhow::Result<()> {
        let ping = MessageUnconnectedPing::deserialize(&mut buf)?;

        let server_uuid = self.server_uuid;
        let motd_payload = match self.motd_reflector.last_motd().await {
            Some(mut motd) => {
                motd.server_uuid = server_uuid;
                motd.port_v4 = self.in_bound_port;
                motd.port_v6 = motd.port_v4;
                if motd.lines[0].is_empty() {
                    // motd reply has no effect with an empty title
                    motd.lines[0] = "...".into();
                }
                motd.encode_payload()
            }
            None => String::new(),
        };

        let pong = MessageUnconnectedPong {
            timestamp: ping.forward_timestamp,
            server_uuid,
            motd: motd_payload,
        };
        self.in_udp_sock.send_to(&pong.to_bytes()?, addr).await?;
        Ok(())
    }

    /// Returns a reference to the metrics counters.
    pub fn metrics(&self) -> &Arc<Metrics> {
        &self.metrics
    }
}

impl RaknetClient {
    /// Sends a packet with HAProxy protocol header.
    async fn send_haproxy_info(&self) -> anyhow::Result<()> {
        let header = haproxy::Builder::with_addresses(
            haproxy::Version::Two | haproxy::Command::Proxy,
            haproxy::Protocol::Datagram,
            (self.addr, self.proxy_udp_sock.local_addr()?),
        )
        .build()?;
        self.udp_sock.send(&header).await?;
        Ok(())
    }

    /// Runs the client event loop with phase-specific timeouts.
    async fn run_event_loop(
        &self,
        mut rx: mpsc::Receiver<DisconnectCause>,
        handshake_timeout: Duration,
        backend_silence_timeout: Duration,
    ) -> anyhow::Result<DisconnectCause> {
        let mut buf = [0u8; 1492];
        loop {
            let timeout = match self.stage() {
                STAGE_HANDSHAKE => handshake_timeout,
                STAGE_CONNECTED => backend_silence_timeout,
                _ => return Ok(DisconnectCause::Unknown),
            };
            tokio::select! {
                cause = rx.recv() => return Ok(cause.unwrap_or(DisconnectCause::Unknown)),

                res = tokio::time::timeout(timeout, self.udp_sock.recv(&mut buf)) => {
                    let len = match res {
                        Ok(res) => res?,
                        Err(_) => return Ok(DisconnectCause::Timeout),
                    };
                    if len == 0 {
                        continue;
                    }
                    self.forward_to_player(&buf[..len]).await;

                    let message_type = RaknetMessage::from_u8(buf[0]);
                    if matches!(message_type, Some(RaknetMessage::OpenConnectionReply2))
                        && self
                            .stage
                            .compare_exchange(
                                STAGE_HANDSHAKE,
                                STAGE_CONNECTED,
                                Ordering::AcqRel,
                                Ordering::Acquire,
                            )
                            .is_ok()
                    {
                        log::info!(
                            "Player {} has connected to {}",
                            self.addr,
                            self.server.addr
                        )
                    }
                    if let Some(ref mt) = message_type {
                        log::trace!(
                            "{} Relaying message {:?}",
                            self.prefix_s2p,
                            mt
                        );
                    }
                    // Spy for disconnect
                    let data = Bytes::copy_from_slice(&buf[..len]);
                    if matches!(
                        self.spy_datagram(Direction::ServerToPlayer, data),
                        Ok(SpyDatagramResult::Disconnect)
                    ) {
                        log::debug!(
                            "{} Found disconnect notification in datagram",
                            self.prefix_s2p,
                        );
                        self.close_tx.send(DisconnectCause::Server).await?;
                    }
                }
            }
        }
    }

    /// Forwards data received from the server to the player.
    ///
    /// ## Arguments
    ///
    /// * `data` - Raw data received from the server
    #[inline]
    async fn forward_to_player(&self, data: &[u8]) {
        if let Err(err) = self.proxy_udp_sock.send_to(data, self.addr).await {
            log::debug!("{} Unable to forward data: {:?}", self.prefix_s2p, err);
        }
    }

    /// Handles incoming data from the UDP socket from the player to the server.
    ///
    /// ## Arguments
    ///
    /// * `data` - Raw received data
    async fn handle_incoming_player(&self, data: Bytes) -> anyhow::Result<()> {
        if data.is_empty() {
            return Ok(());
        }
        if data[0] & 0x80 == 0 {
            log::trace!(
                "{} Received non-datagram data, with header {:02x}",
                self.prefix_p2s,
                data[0]
            );
            // while this is technically invalid,
            // not forwarding it would make the proxy inconsistent
            self.forward_to_server(&data).await;
            return Ok(());
        }
        self.forward_to_server(&data).await;
        if matches!(
            self.spy_datagram(Direction::PlayerToServer, data),
            Ok(SpyDatagramResult::Disconnect)
        ) {
            log::debug!(
                "{} Found disconnect notification in datagram",
                self.prefix_p2s,
            );
            self.close_tx.send(DisconnectCause::Client).await?;
        }
        Ok(())
    }

    /// Spies a datagram to look for a disconnect notification.
    ///
    /// Since we are looking for something specific and don't want to incur too much overhead anyway,
    /// the frames are partially decoded, only non-fragmented frames are read given this is what a disconnect
    /// notification message will be wrapped into.
    /// We don't need to bother with frame (re-)ordering either.
    ///
    /// ## Arguments
    ///
    /// * `direction` - Data flow direction
    /// * `data` - Datagram received data
    fn spy_datagram(&self, direction: Direction, data: Bytes) -> anyhow::Result<SpyDatagramResult> {
        let mut buf = ReadBuf::new(data);
        let _ = buf.read_u8()?; // header flags
        let _ = buf.read_u24()?; // seq
        while buf.0.has_remaining() {
            let frame = Frame::deserialize(&mut buf)?;
            if frame.fragment.is_some() || frame.body.is_empty() {
                continue;
            }
            if frame.body[0] == raknet::GAME_PACKET_HEADER {
                // we could spy into game packets to look for a Disconnect packet but it may not really be worth it
                // what happens currently is that when the client receives a Disconnect packet it closes the connection
                // and never sends an ACK, so the server tries to send the packet in a loop for a few seconds
                // it's pretty negligible, I don't think it matters much
                continue;
            }
            let message_type = RaknetMessage::from_u8(frame.body[0]);
            let prefix = match direction {
                Direction::PlayerToServer => &self.prefix_p2s,
                Direction::ServerToPlayer => &self.prefix_s2p,
            };
            log::trace!(
                "{} Frame with message type {:?} ({:02x}) and body size {}",
                prefix,
                message_type,
                frame.body[0],
                frame.body.len(),
            );
            if matches!(message_type, Some(RaknetMessage::DisconnectNotification)) {
                return Ok(SpyDatagramResult::Disconnect);
            }
        }
        Ok(SpyDatagramResult::Ignore)
    }

    /// Forwards data received from the player to the server.
    ///
    /// ## Arguments
    ///
    /// * `data` - Raw data received from the player
    #[inline]
    async fn forward_to_server(&self, data: &[u8]) {
        if let Err(err) = self.udp_sock.send(data).await {
            log::debug!("{} Unable to forward data: {:?}", self.prefix_p2s, err);
        }
    }
}

impl fmt::Display for DisconnectCause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // We don't know whether the server sent a Disconnect GAME packet,
            // and the first disconnect notification that will be seen will be from the client.
            // Since I don't want to have to spy inside GAME packets (compression, encryption,
            // incur CPU cost, etc) it will most likely remain like this.
            Self::Client => write!(f, "normal"),
            Self::Server => write!(f, "server"),
            Self::Timeout => write!(f, "timeout"),
            Self::Error => write!(f, "unexpected error"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}
