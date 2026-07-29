use atem_protocol::{
    build_client_handshake, build_server_handshake_reply, decode_packet, server_session_id,
    PacketFlags, HANDSHAKE_CLIENT_STATUS, HANDSHAKE_OK_STATUS,
};

/// Golden-ish check that handshake packets match expected sizes and status bytes.
#[test]
fn handshake_fixture_shape() {
    let client = build_client_handshake(0x1234);
    assert_eq!(client.len(), 20);
    let (h, payload) = decode_packet(&client).unwrap();
    assert!(h.flags.contains(PacketFlags::HANDSHAKE));
    assert_eq!(payload[0], HANDSHAKE_CLIENT_STATUS);

    let assigned = server_session_id(5);
    let reply = build_server_handshake_reply(assigned);
    assert_eq!(reply.len(), 20);
    let (rh, rpayload) = decode_packet(&reply).unwrap();
    assert!(rh.flags.contains(PacketFlags::HANDSHAKE));
    assert_eq!(rpayload[0], HANDSHAKE_OK_STATUS);
    assert_eq!(rh.session_id, assigned);
}

#[test]
fn recorded_client_hello_hex_parses() {
    // LibAtem-style client HELLO (session 0x0001)
    let hex = "1014000100000000006800000100000800000000";
    let bytes = hex::decode(hex).unwrap();
    let (h, payload) = decode_packet(&bytes).unwrap();
    assert!(h.flags.contains(PacketFlags::HANDSHAKE));
    assert_eq!(h.session_id, 1);
    assert_eq!(payload[0], 0x01);
}
