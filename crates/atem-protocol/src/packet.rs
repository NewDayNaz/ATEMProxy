use crate::error::ProtocolError;
use bitflags::bitflags;

pub const ATEM_UDP_PORT: u16 = 9910;
pub const HEADER_LEN: usize = 12;
/// Practical max payload packing size used by LibAtem (~1400 total).
pub const MAX_PACKET_LEN: usize = 1400;

bitflags! {
    /// Flags live in the high 5 bits of the first big-endian u16 (with length in low 11).
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct PacketFlags: u8 {
        const ACK_REQUEST = 0x01;
        const HANDSHAKE = 0x02;
        const IS_RETRANSMIT = 0x04;
        const RETRANSMIT_REQUEST = 0x08;
        const ACK_REPLY = 0x10;
    }
}

/// 12-byte ATEM UDP header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PacketHeader {
    pub flags: PacketFlags,
    pub length: u16,
    pub session_id: u16,
    pub ack_id: u16,
    pub retransmit_request: u16,
    pub unknown: u16,
    pub packet_id: u16,
}

impl PacketHeader {
    pub fn new(flags: PacketFlags, session_id: u16, length: u16) -> Self {
        Self {
            flags,
            length,
            session_id,
            ack_id: 0,
            retransmit_request: 0,
            unknown: 0,
            packet_id: 0,
        }
    }

    pub fn encode(&self, out: &mut [u8; HEADER_LEN]) {
        let flags_len = ((self.flags.bits() as u16) << 11) | (self.length & 0x07FF);
        out[0..2].copy_from_slice(&flags_len.to_be_bytes());
        out[2..4].copy_from_slice(&self.session_id.to_be_bytes());
        out[4..6].copy_from_slice(&self.ack_id.to_be_bytes());
        out[6..8].copy_from_slice(&self.retransmit_request.to_be_bytes());
        out[8..10].copy_from_slice(&self.unknown.to_be_bytes());
        out[10..12].copy_from_slice(&self.packet_id.to_be_bytes());
    }

    pub fn decode(buf: &[u8]) -> Result<Self, ProtocolError> {
        if buf.len() < HEADER_LEN {
            return Err(ProtocolError::TooShort(buf.len()));
        }
        let flags_len = u16::from_be_bytes([buf[0], buf[1]]);
        let flags = PacketFlags::from_bits_truncate((flags_len >> 11) as u8);
        let length = flags_len & 0x07FF;
        Ok(Self {
            flags,
            length,
            session_id: u16::from_be_bytes([buf[2], buf[3]]),
            ack_id: u16::from_be_bytes([buf[4], buf[5]]),
            retransmit_request: u16::from_be_bytes([buf[6], buf[7]]),
            unknown: u16::from_be_bytes([buf[8], buf[9]]),
            packet_id: u16::from_be_bytes([buf[10], buf[11]]),
        })
    }
}

pub fn decode_packet(buf: &[u8]) -> Result<(PacketHeader, &[u8]), ProtocolError> {
    let header = PacketHeader::decode(buf)?;
    let declared = header.length as usize;
    if declared < HEADER_LEN {
        return Err(ProtocolError::LengthMismatch {
            declared,
            actual: buf.len(),
        });
    }
    if buf.len() < declared {
        return Err(ProtocolError::LengthMismatch {
            declared,
            actual: buf.len(),
        });
    }
    Ok((header, &buf[HEADER_LEN..declared]))
}

pub fn encode_packet(header: &PacketHeader, payload: &[u8]) -> Vec<u8> {
    let mut h = *header;
    h.length = (HEADER_LEN + payload.len()) as u16;
    let mut out = vec![0u8; h.length as usize];
    let mut hdr = [0u8; HEADER_LEN];
    h.encode(&mut hdr);
    out[..HEADER_LEN].copy_from_slice(&hdr);
    out[HEADER_LEN..].copy_from_slice(payload);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_header_flags_and_length() {
        let mut h = PacketHeader::new(
            PacketFlags::ACK_REQUEST | PacketFlags::ACK_REPLY,
            0x8008,
            12,
        );
        h.ack_id = 42;
        h.packet_id = 7;
        let mut buf = [0u8; HEADER_LEN];
        h.encode(&mut buf);
        let decoded = PacketHeader::decode(&buf).unwrap();
        assert_eq!(decoded.flags, h.flags);
        assert_eq!(decoded.length, 12);
        assert_eq!(decoded.session_id, 0x8008);
        assert_eq!(decoded.ack_id, 42);
        assert_eq!(decoded.packet_id, 7);
    }

    #[test]
    fn encode_packet_sets_length() {
        let h = PacketHeader::new(PacketFlags::HANDSHAKE, 0x1234, 0);
        let pkt = encode_packet(&h, &[1, 2, 3, 4]);
        assert_eq!(pkt.len(), 16);
        let (dh, payload) = decode_packet(&pkt).unwrap();
        assert_eq!(dh.length, 16);
        assert_eq!(payload, &[1, 2, 3, 4]);
    }
}
