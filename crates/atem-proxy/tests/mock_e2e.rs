//! End-to-end smoke test with a mock ATEM and two mock clients.

use atem_protocol::{
    build_ack_reply, build_client_handshake, build_server_handshake_reply, decode_packet,
    encode_packet, is_handshake_packet, parse_commands, serialize_command, CommandName,
    PacketFlags, PacketHeader, ATEM_UDP_PORT, HANDSHAKE_OK_STATUS,
};
use atem_proxy::{run_proxy, Config};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;

async fn mock_atem(sock: UdpSocket, forwarded_cuts: Arc<AtomicUsize>) {
    let mut buf = [0u8; 2048];
    let session = 0x8001u16;
    let mut next_id = 1u16;
    let mut dumped = false;
    loop {
        let Ok((n, peer)) = sock.recv_from(&mut buf).await else {
            break;
        };
        let Ok((hdr, payload)) = decode_packet(&buf[..n]) else {
            continue;
        };
        if is_handshake_packet(hdr.flags) {
            let reply = build_server_handshake_reply(session);
            let _ = sock.send_to(&reply, peer).await;
            dumped = false;
            next_id = 1;
            continue;
        }
        // After client ACK / any traffic, send init dump once (first reliable id is 1).
        if !dumped {
            let mut body = serialize_command(CommandName(*b"_ver"), &[0, 2, 0, 30]);
            body.extend(serialize_command(CommandName(*b"PrgI"), &[0, 0, 0, 1]));
            body.extend(serialize_command(CommandName(*b"InCm"), &[0x01, 0x00, 0x00, 0x00]));
            let mut h =
                PacketHeader::new(PacketFlags::ACK_REQUEST, session, (12 + body.len()) as u16);
            h.packet_id = next_id;
            next_id = (next_id + 1) % (1 << 15);
            let pkt = encode_packet(&h, &body);
            let _ = sock.send_to(&pkt, peer).await;
            dumped = true;
        }
        if !payload.is_empty() {
            if let Ok(cmds) = parse_commands(payload) {
                for cmd in cmds {
                    if cmd.name.0 == *b"DCut" {
                        forwarded_cuts.fetch_add(1, Ordering::SeqCst);
                    }
                }
            }
        }
        if hdr.flags.contains(PacketFlags::ACK_REQUEST)
            && !hdr.flags.contains(PacketFlags::HANDSHAKE)
        {
            let ack = build_ack_reply(session, hdr.packet_id, 0);
            let _ = sock.send_to(&ack, peer).await;
        }
    }
}

async fn client_handshake(proxy: SocketAddr) -> usize {
    let sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    sock.connect(proxy).await.unwrap();
    let hello = build_client_handshake(0x1111);
    sock.send(&hello).await.unwrap();

    let mut buf = [0u8; 2048];
    let mut commands = 0usize;
    let mut session = 0u16;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while tokio::time::Instant::now() < deadline {
        let Ok(Ok(n)) = tokio::time::timeout(Duration::from_millis(200), sock.recv(&mut buf)).await
        else {
            continue;
        };
        let Ok((hdr, payload)) = decode_packet(&buf[..n]) else {
            continue;
        };
        if is_handshake_packet(hdr.flags) {
            assert_eq!(payload.first().copied(), Some(HANDSHAKE_OK_STATUS));
            assert!(
                hdr.session_id & 0x8000 != 0,
                "session {:#06x}",
                hdr.session_id
            );
            session = hdr.session_id;
            let ack = build_ack_reply(session, 0, 0);
            sock.send(&ack).await.unwrap();
            continue;
        }
        if hdr.flags.contains(PacketFlags::ACK_REQUEST) {
            let ack = build_ack_reply(session, hdr.packet_id, 0);
            let _ = sock.send(&ack).await;
        }
        if !payload.is_empty() {
            if let Ok(cmds) = parse_commands(payload) {
                commands += cmds.len();
                if cmds.iter().any(|c| c.name.0 == *b"InCm") {
                    break;
                }
            }
        }
    }
    commands
}

/// Companion-like client: HELLO, ACK dump, then send DCut as reliable packet id 1.
async fn client_send_cut(proxy: SocketAddr) -> bool {
    let sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    sock.connect(proxy).await.unwrap();
    let hello = build_client_handshake(0x2222);
    sock.send(&hello).await.unwrap();

    let mut buf = [0u8; 2048];
    let mut session = 0u16;
    let mut got_incm = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while tokio::time::Instant::now() < deadline {
        let Ok(Ok(n)) = tokio::time::timeout(Duration::from_millis(200), sock.recv(&mut buf)).await
        else {
            continue;
        };
        let Ok((hdr, payload)) = decode_packet(&buf[..n]) else {
            continue;
        };
        if is_handshake_packet(hdr.flags) {
            session = hdr.session_id;
            let ack = build_ack_reply(session, 0, 0);
            sock.send(&ack).await.unwrap();
            continue;
        }
        if hdr.flags.contains(PacketFlags::ACK_REQUEST) {
            let ack = build_ack_reply(session, hdr.packet_id, 0);
            let _ = sock.send(&ack).await;
        }
        if !payload.is_empty() {
            if let Ok(cmds) = parse_commands(payload) {
                if cmds.iter().any(|c| c.name.0 == *b"InCm") {
                    got_incm = true;
                    break;
                }
            }
        }
    }
    if !got_incm || session == 0 {
        return false;
    }

    // Companion-style: first client reliable packet id is 1, not 0.
    let cut = serialize_command(CommandName(*b"DCut"), &[0, 0]);
    let mut h = PacketHeader::new(
        PacketFlags::ACK_REQUEST,
        session,
        (12 + cut.len()) as u16,
    );
    h.packet_id = 1;
    sock.send(&encode_packet(&h, &cut)).await.unwrap();

    // Yield so the proxy task can recv/forward on multi-thread or current-thread runtimes.
    for _ in 0..20 {
        tokio::time::sleep(Duration::from_millis(25)).await;
        while let Ok(Ok(n)) = tokio::time::timeout(Duration::from_millis(1), sock.recv(&mut buf)).await
        {
            if let Ok((hdr, _)) = decode_packet(&buf[..n]) {
                if hdr.flags.contains(PacketFlags::ACK_REQUEST) {
                    let ack = build_ack_reply(session, hdr.packet_id, 0);
                    let _ = sock.send(&ack).await;
                }
            }
        }
    }
    true
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_clients_receive_init_dump_from_proxy() {
    let atem_sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let atem_addr = atem_sock.local_addr().unwrap();
    let cuts = Arc::new(AtomicUsize::new(0));
    tokio::spawn(mock_atem(atem_sock, cuts.clone()));

    let proxy_sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let proxy_addr = proxy_sock.local_addr().unwrap();
    drop(proxy_sock); // free port for proxy bind

    let cancel = CancellationToken::new();
    let cancel_c = cancel.clone();
    let cfg = Config {
        atem: atem_addr.to_string(),
        bind: proxy_addr,
        log: "error".into(),
        client_idle_ms: 5000,
        reconnect_ms: 200,
        ..Default::default()
    };

    let proxy_task = tokio::spawn(async move {
        let _ = run_proxy(cfg, cancel_c).await;
    });

    // Retry until upstream is ready and dump flows.
    let mut c1 = 0;
    for _ in 0..20 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        c1 = client_handshake(proxy_addr).await;
        if c1 >= 1 {
            break;
        }
    }
    let c2 = client_handshake(proxy_addr).await;
    assert!(c1 >= 1, "client1 expected init commands, got {c1}");
    assert!(c2 >= 1, "client2 expected init commands, got {c2}");

    let mut sent = false;
    for _ in 0..10 {
        if client_send_cut(proxy_addr).await {
            sent = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(sent, "failed to complete client handshake before cut");
    assert!(
        cuts.load(Ordering::SeqCst) >= 1,
        "Companion-style packet_id=1 cut was not forwarded upstream"
    );

    cancel.cancel();
    let _ = proxy_task.await;
    let _ = ATEM_UDP_PORT;
}
