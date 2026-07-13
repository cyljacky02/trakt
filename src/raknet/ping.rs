use std::{
    sync::Arc,
    time::{self, Duration, SystemTime},
};

use anyhow::Context;
use bytes::Bytes;
use tokio::{
    net::{ToSocketAddrs, UdpSocket},
    time::Instant,
};

use crate::raknet::{
    datatypes::ReadBuf,
    message::{Message, MessageUnconnectedPing, RaknetMessage},
};

use super::message::MessageUnconnectedPong;
use ppp::v2 as haproxy;

/// Structured bedrock MOTD representation.
#[derive(Clone, Debug)]
pub struct Motd {
    /// UUID of the server
    pub server_uuid: i64,
    /// Edition of the game run by the server
    pub edition: BedrockEdition,
    /// Protocol version of the server
    pub protocol_version: u16,
    /// Display name of the server version
    pub version_name: String,

    /// MOTD Custom Text (two lines)
    pub lines: [String; 2],
    /// Online player count
    pub player_count: usize,
    /// Maximum player count
    pub max_player_count: usize,
    /// Gamemode.
    pub gamemode: GameMode,
    /// Nintendo limited.
    pub nintendo_limited: bool,

    /// Server port (IPv4)
    pub port_v4: u16,
    /// Server port (IPv6)
    pub port_v6: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BedrockEdition {
    PocketEdition,
    EducationEdition,
    Custom(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GameMode {
    Survival,
    Creative,
    Custom(String),
}

impl Motd {
    /// Encodes a MOTD into a string payload that clients understand.
    pub fn encode_payload(&self) -> String {
        let edition = match &self.edition {
            BedrockEdition::PocketEdition => "MCPE",
            BedrockEdition::EducationEdition => "MCBE",
            BedrockEdition::Custom(str) => str,
        };
        let gamemode = match &self.gamemode {
            GameMode::Survival => "Survival",
            GameMode::Creative => "Creative",
            GameMode::Custom(str) => str,
        };
        format!(
            "{edition};{};{};{};{};{};{};{};{gamemode};{};{};{};",
            self.lines[0],
            self.protocol_version,
            self.version_name,
            self.player_count,
            self.max_player_count,
            self.server_uuid,
            self.lines[1],
            (!self.nintendo_limited) as usize,
            self.port_v4,
            self.port_v6
        )
    }

    /// Decodes a MOTD payload.
    pub fn decode_payload(payload: &str) -> Option<Motd> {
        let mut parts = payload.split(';');
        let edition = match parts.next() {
            Some("MCPE") => BedrockEdition::PocketEdition,
            Some("MCBE") => BedrockEdition::EducationEdition,
            Some(custom) if !custom.is_empty() => BedrockEdition::Custom(custom.to_owned()),
            _ => return None,
        };
        let line_one = parts.next().map(str::to_owned).unwrap_or_default();
        let protocol_version = parts
            .next()
            .and_then(|ver| ver.parse::<u16>().ok())
            .unwrap_or(0);
        let version_name = parts.next().map(str::to_owned).unwrap_or_default();
        let player_count = parts
            .next()
            .and_then(|ver| ver.parse::<usize>().ok())
            .unwrap_or(0);
        let max_player_count = parts
            .next()
            .and_then(|ver| ver.parse::<usize>().ok())
            .unwrap_or(0);
        let server_uuid = parts
            .next()
            .and_then(|ver| ver.parse::<i64>().ok())
            .unwrap_or(0);
        let line_two = parts.next().map(str::to_owned).unwrap_or_default();
        let gamemode = match parts.next() {
            Some("Survival") | None => GameMode::Survival,
            Some("Creative") => GameMode::Creative,
            Some(custom) => GameMode::Custom(custom.to_owned()),
        };
        let nintendo_limited = parts
            .next()
            .and_then(|ver| ver.parse::<i8>().ok())
            .map(|i| i == 0)
            .unwrap_or(false);
        let port_v4 = parts
            .next()
            .and_then(|ver| ver.parse::<u16>().ok())
            .unwrap_or(0);
        let port_v6 = parts
            .next()
            .and_then(|ver| ver.parse::<u16>().ok())
            .unwrap_or(0);
        Some(Motd {
            server_uuid,
            edition,
            protocol_version,
            version_name,
            lines: [line_one, line_two],
            player_count,
            max_player_count,
            gamemode,
            nintendo_limited,
            port_v4,
            port_v6,
        })
    }
}

/// Pings a Bedrock server and get MOTD information.
///
/// ## Arguments
///
/// * `local_addr` - Local address to bind the UDP socket to
/// * `addr` - Address of the remote server
/// * `proxy_protocol` - Whether proxy protocol is required by the server
/// * `timeout` - Timeout duration
pub async fn ping<A1: ToSocketAddrs, A2: ToSocketAddrs>(
    local_addr: A1,
    addr: A2,
    proxy_protocol: bool,
    timeout: Duration,
) -> anyhow::Result<Motd> {
    let udp_sock = UdpSocket::bind(local_addr).await?;
    udp_sock.connect(addr).await?;

    let now = SystemTime::now()
        .duration_since(time::UNIX_EPOCH)?
        .as_secs()
        .try_into()?;
    let ping = MessageUnconnectedPing {
        client_uuid: now,
        forward_timestamp: now,
    };

    let ping_packet = if proxy_protocol {
        let local_addr = udp_sock.local_addr()?;
        let header = haproxy::Builder::with_addresses(
            haproxy::Version::Two | haproxy::Command::Proxy,
            haproxy::Protocol::Datagram,
            (local_addr, local_addr),
        )
        .build()?;

        let mut buf = header;
        buf.extend(ping.to_bytes()?);
        buf
    } else {
        ping.to_bytes()?
    };

    let udp_sock = Arc::new(udp_sock);
    let udp_sock_2 = udp_sock.clone();
    let deadline = Instant::now() + timeout;

    let mut buf = [0u8; 1492];
    let len = tokio::select! {
        res = ping_resender(udp_sock_2, &ping_packet) => {
            res?;
            0
        }
        res = tokio::time::timeout_at(deadline, udp_sock.recv(&mut buf)) => res??,
    };
    let buf = &buf[..len];
    let mut buf = ReadBuf::new(Bytes::copy_from_slice(buf));
    let message_type = RaknetMessage::from_u8(buf.read_u8()?);
    if !matches!(message_type, Some(RaknetMessage::UnconnectedPong)) {
        return Err(anyhow::anyhow!("Received a reply other than pong"));
    }
    let pong = MessageUnconnectedPong::deserialize(&mut buf)?;
    let motd = Motd::decode_payload(&pong.motd).context("empty payload")?;
    Ok(motd)
}

/// Pings a server honoring `proxy_protocol`, retrying once without proxy
/// protocol when the wrapped ping fails.
///
/// Geyser sometimes ignores proxy-protocol-wrapped `UnconnectedPing` packets,
/// so a proxy-protocol-enabled backend that does not answer the wrapped ping
/// may still answer a raw one. Without this fallback such a backend would be
/// reported dead by health checks and MOTD-less, even though real client
/// traffic works.
///
/// # Arguments
///
/// * `local_addr` - Local address to bind to for the ping
/// * `addr` - Address of the server to ping
/// * `proxy_protocol` - Whether proxy protocol is required by the server
/// * `timeout` - Timeout duration for each attempt
pub async fn ping_with_fallback<A1: ToSocketAddrs + Copy, A2: ToSocketAddrs + Copy>(
    local_addr: A1,
    addr: A2,
    proxy_protocol: bool,
    timeout: Duration,
) -> anyhow::Result<Motd> {
    let result = ping(local_addr, addr, proxy_protocol, timeout).await;
    if result.is_err() && proxy_protocol {
        tracing::debug!("Proxy-protocol ping failed, retrying without proxy protocol");
        return ping(local_addr, addr, false, timeout).await;
    }
    result
}

async fn ping_resender(udp_sock: Arc<UdpSocket>, ping_packet: &[u8]) -> anyhow::Result<()> {
    let mut attempts = 0;
    loop {
        attempts += 1;
        tracing::trace!(attempt = attempts, peer = %udp_sock.peer_addr()?, "Ping attempt");
        udp_sock.send(ping_packet).await?;
        tokio::time::sleep(Duration::from_millis(750)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PROXY protocol v2 signature prefixing every wrapped datagram.
    const PP_V2_SIGNATURE: [u8; 12] = [
        0x0D, 0x0A, 0x0D, 0x0A, 0x00, 0x0D, 0x0A, 0x51, 0x55, 0x49, 0x54, 0x0A,
    ];

    fn test_motd() -> Motd {
        Motd {
            server_uuid: 42,
            edition: BedrockEdition::PocketEdition,
            protocol_version: 800,
            version_name: "1.21.0".to_string(),
            lines: ["Geyser-like".to_string(), "backend".to_string()],
            player_count: 0,
            max_player_count: 10,
            gamemode: GameMode::Survival,
            nintendo_limited: false,
            port_v4: 19132,
            port_v6: 19133,
        }
    }

    /// Fake backend that ignores proxy-protocol-wrapped pings (as Geyser
    /// sometimes does) but answers raw `UnconnectedPing`s with a pong.
    async fn spawn_pp_ignoring_backend() -> anyhow::Result<String> {
        let sock = UdpSocket::bind("127.0.0.1:0").await?;
        let addr = sock.local_addr()?.to_string();
        tokio::spawn(async move {
            let mut buf = [0u8; 1492];
            loop {
                let Ok((len, peer)) = sock.recv_from(&mut buf).await else {
                    return;
                };
                if buf[..len].starts_with(&PP_V2_SIGNATURE) {
                    // Geyser-like behavior: silently drop wrapped pings.
                    continue;
                }
                let pong = MessageUnconnectedPong {
                    timestamp: 0,
                    server_uuid: 42,
                    motd: test_motd().encode_payload(),
                };
                let reply = pong.to_bytes().expect("pong encodes");
                let _ = sock.send_to(&reply, peer).await;
            }
        });
        Ok(addr)
    }

    #[tokio::test]
    async fn fallback_reaches_backend_that_ignores_proxy_protocol_pings() {
        let addr = spawn_pp_ignoring_backend().await.expect("backend starts");
        let timeout = Duration::from_secs(2);

        // The regression condition: a plain proxy-protocol ping never gets an
        // answer from a backend that drops wrapped pings.
        let direct = ping("127.0.0.1:0", addr.as_str(), true, timeout).await;
        assert!(direct.is_err(), "wrapped ping should be dropped");

        // With the fallback, the same backend is reachable again.
        let motd = ping_with_fallback("127.0.0.1:0", &addr, true, timeout)
            .await
            .expect("fallback ping succeeds against PP-ignoring backend");
        assert_eq!(motd.server_uuid, 42);
    }

    #[tokio::test]
    async fn plain_ping_still_works_without_proxy_protocol() {
        let addr = spawn_pp_ignoring_backend().await.expect("backend starts");
        let timeout = Duration::from_secs(2);
        let motd = ping_with_fallback("127.0.0.1:0", &addr, false, timeout)
            .await
            .expect("raw ping succeeds");
        assert_eq!(motd.server_uuid, 42);
    }
}
