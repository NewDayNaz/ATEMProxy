use crate::packet::{encode_packet, PacketFlags, PacketHeader, HEADER_LEN};

/// Client HELLO payload status byte.
pub const HANDSHAKE_CLIENT_STATUS: u8 = 0x01;
/// Switcher/proxy HELLO OK status.
pub const HANDSHAKE_OK_STATUS: u8 = 0x02;
/// Special remote-seq value clients send after init (OpenSwitcher).
pub const INIT_ACK_REMOTE_SEQ: u16 = 0x61;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandshakeRole {
    Client,
    Server,
}

/// Assign server-side session IDs in the ATEM style: high bit set.
pub fn server_session_id(index: u16) -> u16 {
    0x8000 | (index & 0x7FFF)
}

pub fn is_handshake_packet(flags: PacketFlags) -> bool {
    flags.contains(PacketFlags::HANDSHAKE)
}

pub fn parse_handshake_status(payload: &[u8]) -> Option<u8> {
    payload.first().copied()
}

/// Client → ATEM/proxy HELLO (20-byte packet).
pub fn build_client_handshake(session_id: u16) -> Vec<u8> {
    let mut header = PacketHeader::new(PacketFlags::HANDSHAKE, session_id, 20);
    header.unknown = 0x0068;
    let payload = [
        HANDSHAKE_CLIENT_STATUS,
        0x00,
        0x00,
        0x08,
        0x00,
        0x00,
        0x00,
        0x00,
    ];
    encode_packet(&header, &payload)
}

/// Proxy/ATEM → client HELLO reply using the **assigned** session id for the whole session.
///
/// Unlike LibAtem AtemProxy (echo client temp id then flip to 0x8008), we keep one id from
/// HELLO through data so SoftAtem/Companion do not see a mid-session identity change.
pub fn build_server_handshake_reply(session_id: u16) -> Vec<u8> {
    let header = PacketHeader::new(PacketFlags::HANDSHAKE, session_id, 20);
    let payload = [
        HANDSHAKE_OK_STATUS,
        0x00,
        0x00,
        0x00,
        0x00,
        0x00,
        0x00,
        0x00,
    ];
    encode_packet(&header, &payload)
}

/// Empty ACK reply packet.
pub fn build_ack_reply(session_id: u16, ack_id: u16, packet_id: u16) -> Vec<u8> {
    let mut header = PacketHeader::new(PacketFlags::ACK_REPLY, session_id, HEADER_LEN as u16);
    header.ack_id = ack_id;
    header.packet_id = packet_id;
    encode_packet(&header, &[])
}

/// Build the special post-init ACK used by clients toward a switcher (remote seq 0x61).
pub fn build_init_complete_ack(session_id: u16, ack_id: u16) -> Vec<u8> {
    let mut header = PacketHeader::new(PacketFlags::ACK_REPLY, session_id, HEADER_LEN as u16);
    header.ack_id = ack_id;
    // OpenSwitcher: write 0x61 into the remote-sequence-related field.
    header.unknown = INIT_ACK_REMOTE_SEQ;
    encode_packet(&header, &[])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::decode_packet;

    #[test]
    fn client_handshake_is_20_bytes() {
        let pkt = build_client_handshake(0x1234);
        assert_eq!(pkt.len(), 20);
        let (h, payload) = decode_packet(&pkt).unwrap();
        assert!(h.flags.contains(PacketFlags::HANDSHAKE));
        assert_eq!(payload[0], HANDSHAKE_CLIENT_STATUS);
    }

    #[test]
    fn server_session_sets_high_bit() {
        assert_eq!(server_session_id(8), 0x8008);
        assert_eq!(server_session_id(1), 0x8001);
    }

    #[test]
    fn server_handshake_uses_assigned_session() {
        let pkt = build_server_handshake_reply(0x8005);
        let (h, payload) = decode_packet(&pkt).unwrap();
        assert_eq!(h.session_id, 0x8005);
        assert_eq!(payload[0], HANDSHAKE_OK_STATUS);
    }
}
