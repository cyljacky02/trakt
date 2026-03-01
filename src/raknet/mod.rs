pub mod datatypes;
pub mod frame;
pub mod message;
pub mod ping;

/// A Raknet GAME packet header (Bedrock protocol layer, usually compressed).
pub const GAME_PACKET_HEADER: u8 = 0xfe;

/// ACK packet ID.
pub const ACK: u8 = 0xc0;

/// NACK packet ID.
pub const NACK: u8 = 0xa0;

/// Offline message marker.
pub(super) const MAGIC: [u8; 16] = [
    0x00, 0xFF, 0xFF, 0x00, 0xFE, 0xFE, 0xFE, 0xFE, 0xFD, 0xFD, 0xFD, 0xFD, 0x12, 0x34, 0x56, 0x78,
];

/// Returns true if the byte is a valid RakNet packet identifier.
///
/// Valid IDs from the RakNet/Bedrock protocol:
/// - `0x00–0x09`: Online/offline messages (ping, pong, handshake, connection)
/// - `0x0a–0x0b`: Security negotiation
/// - `0x10–0x17, 0x19, 0x1c`: Connection management + pong
/// - `0x80–0x8d`: Frame Set Packets (datagrams)
/// - `0xa0`: NACK
/// - `0xc0`: ACK
/// - `0xfe`: Game packet
#[inline]
pub fn is_valid_raknet_byte(b: u8) -> bool {
    matches!(
        b,
        0x00..=0x0b
            | 0x10..=0x17
            | 0x19
            | 0x1c
            | 0x80..=0x8d
            | ACK
            | NACK
            | GAME_PACKET_HEADER
    )
}

/// Returns true if the byte is a valid Frame Set Packet (datagram).
#[inline]
pub fn is_datagram(b: u8) -> bool {
    (0x80..=0x8d).contains(&b)
}

/// Returns true if the byte is a valid connected-session packet
/// (datagram, ACK, NACK, or game packet).
#[inline]
pub fn is_connected_traffic(b: u8) -> bool {
    is_datagram(b) || b == ACK || b == NACK || b == GAME_PACKET_HEADER
}
