use crate::packet::{encode_packet, PacketFlags, PacketHeader, HEADER_LEN, MAX_PACKET_LEN};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// 15-bit packet ID space used by ATEM.
pub const MAX_PACKET_ID: u16 = 1 << 15;

#[derive(Debug, Clone, Copy)]
pub struct ReliableConfig {
    pub max_in_flight: usize,
    pub retransmit_after: Duration,
    pub ack_coalesce_count: u16,
    pub ack_coalesce_after: Duration,
    pub ping_interval: Duration,
    pub idle_timeout: Duration,
}

impl Default for ReliableConfig {
    fn default() -> Self {
        Self {
            max_in_flight: 32,
            retransmit_after: Duration::from_millis(30),
            ack_coalesce_count: 16,
            ack_coalesce_after: Duration::from_millis(5),
            ping_interval: Duration::from_millis(100),
            idle_timeout: Duration::from_millis(1000),
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SeqSpace;

pub fn next_packet_id(current: u16) -> u16 {
    (current + 1) % MAX_PACKET_ID
}

/// Wrap-aware check whether `ack_id` covers `packet_id` (LibAtem-style ±256 window).
pub fn is_ack_covering(ack_id: u16, packet_id: u16) -> bool {
    if ack_id == packet_id {
        return true;
    }
    let diff = ((ack_id as i32) - (packet_id as i32) + MAX_PACKET_ID as i32) % MAX_PACKET_ID as i32;
    diff < 256
}

#[derive(Debug, Clone)]
pub struct InFlightEntry {
    pub packet_id: u16,
    pub bytes: Vec<u8>,
    pub sent_at: Instant,
}

#[derive(Debug, Default)]
pub struct AckState {
    pub highest_recv: Option<u16>,
    pub expected_recv: u16,
    pub last_ack_sent_at: Option<Instant>,
    pub packets_since_ack: u16,
    pub need_init_ack: bool,
}

/// Per-connection reliable UDP state (client or server role).
#[derive(Debug)]
pub struct ReliableEndpoint {
    pub session_id: u16,
    pub next_send_id: u16,
    pub config: ReliableConfig,
    pub in_flight: VecDeque<InFlightEntry>,
    pub outbound: VecDeque<Vec<u8>>,
    pub ack: AckState,
    pub last_recv_at: Instant,
    pub last_ping_at: Instant,
    pub opened: bool,
}

impl ReliableEndpoint {
    pub fn new(session_id: u16, config: ReliableConfig) -> Self {
        let now = Instant::now();
        Self {
            session_id,
            next_send_id: 0,
            config,
            in_flight: VecDeque::new(),
            outbound: VecDeque::new(),
            ack: AckState::default(),
            last_recv_at: now,
            last_ping_at: now,
            opened: false,
        }
    }

    pub fn note_recv(&mut self) {
        self.last_recv_at = Instant::now();
    }

    pub fn is_timed_out(&self) -> bool {
        self.last_recv_at.elapsed() > self.config.idle_timeout
    }

    pub fn queue_payload(&mut self, payload: Vec<u8>) {
        self.outbound.push_back(payload);
    }

    pub fn queue_commands_packed(&mut self, commands: &[Vec<u8>]) {
        let mut current = Vec::new();
        for cmd in commands {
            if HEADER_LEN + current.len() + cmd.len() > MAX_PACKET_LEN {
                if !current.is_empty() {
                    self.outbound.push_back(std::mem::take(&mut current));
                }
            }
            if HEADER_LEN + cmd.len() > MAX_PACKET_LEN {
                // Oversized single command: send alone (may exceed MTU; rare).
                self.outbound.push_back(cmd.clone());
                continue;
            }
            current.extend_from_slice(cmd);
        }
        if !current.is_empty() {
            self.outbound.push_back(current);
        }
    }

    pub fn on_ack_id(&mut self, ack_id: u16) {
        while let Some(front) = self.in_flight.front() {
            if is_ack_covering(ack_id, front.packet_id) {
                self.in_flight.pop_front();
            } else {
                break;
            }
        }
    }

    /// Returns true if the packet should be accepted for in-order delivery.
    pub fn accept_reliable(&mut self, packet_id: u16, is_retransmit: bool) -> bool {
        if packet_id == self.ack.expected_recv {
            self.ack.expected_recv = next_packet_id(self.ack.expected_recv);
            self.ack.highest_recv = Some(packet_id);
            self.ack.packets_since_ack = self.ack.packets_since_ack.saturating_add(1);
            true
        } else if is_retransmit && is_ack_covering(self.ack.expected_recv.wrapping_sub(1) % MAX_PACKET_ID, packet_id)
        {
            // Duplicate retransmit of already-seen packet.
            false
        } else {
            // Out of order — drop for now; caller may send retransmit request.
            false
        }
    }

    pub fn should_send_ack(&self) -> bool {
        if self.ack.highest_recv.is_none() {
            return false;
        }
        if self.ack.packets_since_ack >= self.config.ack_coalesce_count {
            return true;
        }
        match self.ack.last_ack_sent_at {
            None => self.ack.packets_since_ack > 0,
            Some(t) => {
                self.ack.packets_since_ack > 0 && t.elapsed() >= self.config.ack_coalesce_after
            }
        }
    }

    pub fn build_ack_packet(&mut self) -> Option<Vec<u8>> {
        let ack_id = self.ack.highest_recv?;
        let mut header =
            PacketHeader::new(PacketFlags::ACK_REPLY, self.session_id, HEADER_LEN as u16);
        header.ack_id = ack_id;
        self.ack.packets_since_ack = 0;
        self.ack.last_ack_sent_at = Some(Instant::now());
        Some(encode_packet(&header, &[]))
    }

    pub fn build_retransmit_request(&self) -> Vec<u8> {
        let mut header = PacketHeader::new(
            PacketFlags::RETRANSMIT_REQUEST,
            self.session_id,
            HEADER_LEN as u16,
        );
        header.retransmit_request = self.ack.expected_recv;
        encode_packet(&header, &[])
    }

    /// Produce next outbound datagrams: retransmits, new reliable packets, acks, pings.
    pub fn poll_outbound(&mut self, now: Instant) -> Vec<Vec<u8>> {
        let mut out = Vec::new();

        // Retransmit entire in-flight window if oldest is stale (LibAtem behavior).
        if let Some(oldest) = self.in_flight.front() {
            if now.duration_since(oldest.sent_at) >= self.config.retransmit_after {
                for entry in &mut self.in_flight {
                    let mut bytes = entry.bytes.clone();
                    if bytes.len() >= 2 {
                        let flags_len = u16::from_be_bytes([bytes[0], bytes[1]]);
                        let flags = ((flags_len >> 11) as u8) | PacketFlags::IS_RETRANSMIT.bits();
                        let length = flags_len & 0x07FF;
                        let new_fl = ((flags as u16) << 11) | length;
                        bytes[0..2].copy_from_slice(&new_fl.to_be_bytes());
                    }
                    entry.sent_at = now;
                    out.push(bytes);
                }
            }
        }

        while self.in_flight.len() < self.config.max_in_flight {
            let Some(payload) = self.outbound.pop_front() else {
                break;
            };
            let packet_id = self.next_send_id;
            self.next_send_id = next_packet_id(self.next_send_id);
            let mut header = PacketHeader::new(
                PacketFlags::ACK_REQUEST,
                self.session_id,
                (HEADER_LEN + payload.len()) as u16,
            );
            header.packet_id = packet_id;
            let bytes = encode_packet(&header, &payload);
            self.in_flight.push_back(InFlightEntry {
                packet_id,
                bytes: bytes.clone(),
                sent_at: now,
            });
            out.push(bytes);
        }

        if self.should_send_ack() {
            if let Some(ack) = self.build_ack_packet() {
                out.push(ack);
            }
        }

        if self.opened && now.duration_since(self.last_ping_at) >= self.config.ping_interval {
            let mut header =
                PacketHeader::new(PacketFlags::ACK_REQUEST, self.session_id, HEADER_LEN as u16);
            // Pings still consume packet IDs in LibAtem when sent as AckRequest empty.
            header.packet_id = self.next_send_id;
            self.next_send_id = next_packet_id(self.next_send_id);
            let bytes = encode_packet(&header, &[]);
            // Track ping in-flight lightly by not requiring ack for empty? LibAtem tracks them.
            self.in_flight.push_back(InFlightEntry {
                packet_id: header.packet_id,
                bytes: bytes.clone(),
                sent_at: now,
            });
            self.last_ping_at = now;
            out.push(bytes);
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packet_id_wraps_at_15_bits() {
        assert_eq!(next_packet_id(MAX_PACKET_ID - 1), 0);
    }

    #[test]
    fn ack_covering_window() {
        assert!(is_ack_covering(10, 10));
        assert!(is_ack_covering(20, 10));
        assert!(!is_ack_covering(10, 20));
    }

    #[test]
    fn packs_commands_under_mtu() {
        let mut ep = ReliableEndpoint::new(0x8001, ReliableConfig::default());
        let cmd = vec![0u8; 100];
        let cmds = vec![cmd; 20];
        ep.queue_commands_packed(&cmds);
        // Should produce multiple outbound payloads
        assert!(ep.outbound.len() > 1);
    }
}
