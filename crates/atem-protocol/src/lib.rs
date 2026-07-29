//! Low-level Blackmagic ATEM UDP protocol helpers.
//!
//! Wire format references: OpenSwitcher UDP transport docs, LibAtem, Wireshark ATEM dissector.

mod command;
mod error;
mod packet;
mod reliable;
mod session;

pub use command::{
    command_identity, is_audio_levels_subscribe, is_ephemeral_command, is_lock_command,
    is_transfer_command, parse_commands, parse_version, serialize_command, synthetic_init_complete,
    CommandName, CommandRef, INIT_COMPLETE,
};
pub use error::ProtocolError;
pub use packet::{
    decode_packet, encode_packet, ATEM_UDP_PORT, HEADER_LEN, MAX_PACKET_LEN, PacketFlags,
    PacketHeader,
};
pub use reliable::{
    is_ack_covering, next_packet_id, AckState, InFlightEntry, ReliableConfig, ReliableEndpoint,
    SeqSpace, MAX_PACKET_ID,
};
pub use session::{
    build_ack_reply, build_client_handshake, build_init_complete_ack, build_server_handshake_reply,
    is_handshake_packet, parse_handshake_status, server_session_id, HandshakeRole,
    HANDSHAKE_CLIENT_STATUS, HANDSHAKE_OK_STATUS, INIT_ACK_REMOTE_SEQ,
};
