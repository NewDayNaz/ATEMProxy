use crate::cache::StateCache;
use crate::filter::{CommandFilter, FilterAction};
use crate::upstream::{UpstreamEvent, UpstreamHandle};
use anyhow::{Context, Result};
use atem_protocol::{
    build_server_handshake_reply, decode_packet, is_handshake_packet, server_session_id,
    PacketFlags, ReliableConfig, ReliableEndpoint,
};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

struct ClientSession {
    rel: ReliableEndpoint,
    dump_sent: bool,
    opened: bool,
    audio: bool,
}

pub async fn run_server(
    bind: SocketAddr,
    cache: Arc<StateCache>,
    upstream: UpstreamHandle,
    filter: Arc<CommandFilter>,
    client_idle_ms: u64,
    cancel: CancellationToken,
) -> Result<()> {
    let sock = Arc::new(
        UdpSocket::bind(bind)
            .await
            .with_context(|| format!("bind client UDP {bind}"))?,
    );
    info!(%bind, "client UDP server listening");

    let sessions: Arc<Mutex<HashMap<SocketAddr, ClientSession>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let next_sid = Arc::new(AtomicU16::new(1));

    // Fan-out task from upstream events
    let mut events = upstream.events.subscribe();
    let sessions_e = sessions.clone();
    let cancel_e = cancel.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = cancel_e.cancelled() => break,
                msg = events.recv() => {
                    match msg {
                        Ok(UpstreamEvent::Connected) => {}
                        Ok(UpstreamEvent::Disconnected) => {
                            sessions_e.lock().clear();
                            info!("cleared client sessions after upstream disconnect");
                        }
                        Ok(UpstreamEvent::StatePayload(payload)) => {
                            let mut g = sessions_e.lock();
                            for sess in g.values_mut() {
                                if sess.opened {
                                    sess.rel.queue_payload(payload.clone());
                                }
                            }
                        }
                        Ok(UpstreamEvent::AudioLevels(payload)) => {
                            let mut g = sessions_e.lock();
                            for sess in g.values_mut() {
                                if sess.opened && sess.audio {
                                    sess.rel.queue_payload(payload.clone());
                                }
                            }
                        }
                        Err(_) => {}
                    }
                }
            }
        }
    });

    // Send / timeout tick
    let sessions_t = sessions.clone();
    let sock_t = sock.clone();
    let filter_t = filter.clone();
    let cancel_t = cancel.clone();
    let idle = Duration::from_millis(client_idle_ms.max(200));
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_millis(5));
        loop {
            tokio::select! {
                _ = cancel_t.cancelled() => break,
                _ = tick.tick() => {
                    let now = Instant::now();
                    let mut dead = Vec::new();
                    let mut packets: Vec<(SocketAddr, Vec<u8>)> = Vec::new();
                    {
                        let mut g = sessions_t.lock();
                        for (addr, sess) in g.iter_mut() {
                            if now.duration_since(sess.rel.last_recv_at) > idle {
                                dead.push(*addr);
                                continue;
                            }
                            for pkt in sess.rel.poll_outbound(now) {
                                packets.push((*addr, pkt));
                            }
                        }
                        for addr in &dead {
                            g.remove(addr);
                        }
                    }
                    for addr in dead {
                        let _ = filter_t.client_disconnected(addr);
                        info!(%addr, "client timed out");
                    }
                    for (addr, pkt) in packets {
                        let _ = sock_t.send_to(&pkt, addr).await;
                    }
                }
            }
        }
    });

    let mut buf = [0u8; 2048];
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            res = sock.recv_from(&mut buf) => {
                let (n, addr) = res?;
                if let Err(e) = handle_datagram(
                    &buf[..n],
                    addr,
                    &sock,
                    &sessions,
                    &next_sid,
                    &cache,
                    &upstream,
                    &filter,
                ).await {
                    warn!(%addr, error = %e, "client packet error");
                }
            }
        }
    }
    Ok(())
}

async fn handle_datagram(
    data: &[u8],
    addr: SocketAddr,
    sock: &UdpSocket,
    sessions: &Mutex<HashMap<SocketAddr, ClientSession>>,
    next_sid: &AtomicU16,
    cache: &StateCache,
    upstream: &UpstreamHandle,
    filter: &CommandFilter,
) -> Result<()> {
    let (hdr, payload) = decode_packet(data)?;

    if is_handshake_packet(hdr.flags) {
        if !upstream.connected.load(Ordering::SeqCst) && !cache.is_ready() {
            debug!(%addr, "rejecting handshake; upstream not ready");
            return Ok(());
        }
        let reply = build_server_handshake_reply(&hdr);
        sock.send_to(&reply, addr).await?;

        let sid_index = next_sid.fetch_add(1, Ordering::SeqCst);
        let session_id = server_session_id(sid_index);
        let mut cfg = ReliableConfig::default();
        cfg.idle_timeout = Duration::from_millis(5_000);
        let mut rel = ReliableEndpoint::new(session_id, cfg);
        rel.note_recv();
        sessions.lock().insert(
            addr,
            ClientSession {
                rel,
                dump_sent: false,
                opened: false,
                audio: false,
            },
        );
        info!(%addr, session = format!("0x{session_id:04x}"), "client handshake");
        return Ok(());
    }

    let mut retransmit: Option<Vec<u8>> = None;
    {
        let mut g = sessions.lock();
        let Some(sess) = g.get_mut(&addr) else {
            debug!(%addr, "packet from unknown client; ignored");
            return Ok(());
        };
        sess.rel.note_recv();

        if hdr.flags.contains(PacketFlags::ACK_REPLY) || hdr.ack_id != 0 {
            sess.rel.on_ack_id(hdr.ack_id);
            if !sess.opened {
                sess.opened = true;
                sess.rel.opened = true;
                if !sess.dump_sent {
                    let dump = cache.dump();
                    sess.rel.queue_commands_packed(&dump);
                    sess.dump_sent = true;
                    info!(%addr, commands = dump.len(), "queued init dump");
                }
            }
        }

        if hdr.flags.contains(PacketFlags::ACK_REQUEST) {
            let ok = sess.rel.accept_reliable(
                hdr.packet_id,
                hdr.flags.contains(PacketFlags::IS_RETRANSMIT),
            );
            if !ok && !payload.is_empty() {
                retransmit = Some(sess.rel.build_retransmit_request());
            } else if !payload.is_empty() {
                let actions = filter.process_payload(addr, payload);
                for action in actions {
                    match action {
                        FilterAction::Forward(raw) => {
                            let _ = upstream.cmd_tx.try_send(raw);
                        }
                        FilterAction::Dropped { .. } => {}
                        FilterAction::AudioSubscribe { enable, forward } => {
                            sess.audio = enable;
                            if let Some(raw) = forward {
                                let _ = upstream.cmd_tx.try_send(raw);
                            }
                        }
                    }
                }
            }
        }
    }
    if let Some(req) = retransmit {
        sock.send_to(&req, addr).await?;
    }
    Ok(())
}
