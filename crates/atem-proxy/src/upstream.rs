use crate::cache::StateCache;
use crate::locks::LockBroker;
use anyhow::{bail, Context, Result};
use atem_protocol::{
    build_client_handshake, build_init_complete_ack, decode_packet, is_ephemeral_command,
    is_handshake_packet, is_lock_status, is_transfer_command, next_packet_id, parse_commands,
    parse_handshake_status, PacketFlags, ReliableConfig, ReliableEndpoint, HANDSHAKE_OK_STATUS,
    INIT_COMPLETE,
};
use bytes::Bytes;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

#[derive(Debug, Clone)]
pub enum UpstreamEvent {
    Connected,
    Disconnected,
    /// Framed command payload for all clients.
    StatePayload(Bytes),
    /// High-rate audio levels for subscribers only.
    AudioLevels(Bytes),
    /// Media transfer traffic — server routes to lock/transfer owner only.
    TransferPayload(Bytes),
}

pub struct UpstreamHandle {
    pub connected: Arc<AtomicBool>,
    pub cmd_tx: mpsc::Sender<Vec<u8>>,
    pub events: broadcast::Sender<UpstreamEvent>,
}

pub fn spawn_upstream(
    atem: SocketAddr,
    cache: Arc<StateCache>,
    locks: Arc<LockBroker>,
    cancel: CancellationToken,
    reconnect_ms: u64,
) -> UpstreamHandle {
    let connected = Arc::new(AtomicBool::new(false));
    let (cmd_tx, mut cmd_rx) = mpsc::channel::<Vec<u8>>(512);
    let (events, _) = broadcast::channel(512);
    let connected_flag = connected.clone();
    let events_tx = events.clone();

    tokio::spawn(async move {
        let base = Duration::from_millis(reconnect_ms.max(200));
        let mut backoff = base;
        while !cancel.is_cancelled() {
            let reached_ready = Arc::new(AtomicBool::new(false));
            match run_session(
                atem,
                &cache,
                &locks,
                &connected_flag,
                &events_tx,
                &mut cmd_rx,
                &cancel,
                &reached_ready,
            )
            .await
            {
                Ok(()) => info!("upstream session closed"),
                Err(e) => error!(error = %e, "upstream session failed"),
            }
            let ok = reached_ready.load(Ordering::SeqCst);
            connected_flag.store(false, Ordering::SeqCst);
            cache.clear();
            locks.clear();
            let _ = events_tx.send(UpstreamEvent::Disconnected);
            if cancel.is_cancelled() {
                break;
            }
            if ok {
                backoff = base;
            }
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = tokio::time::sleep(backoff) => {}
            }
            if !ok {
                backoff = (backoff * 2).min(Duration::from_secs(30));
            }
        }
    });

    UpstreamHandle {
        connected,
        cmd_tx,
        events,
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_session(
    atem: SocketAddr,
    cache: &StateCache,
    locks: &LockBroker,
    connected: &AtomicBool,
    events: &broadcast::Sender<UpstreamEvent>,
    cmd_rx: &mut mpsc::Receiver<Vec<u8>>,
    cancel: &CancellationToken,
    reached_ready: &AtomicBool,
) -> Result<()> {
    let sock = UdpSocket::bind("0.0.0.0:0")
        .await
        .context("bind upstream UDP socket")?;
    sock.connect(atem)
        .await
        .with_context(|| format!("connect to ATEM {atem}"))?;
    info!(%atem, local = ?sock.local_addr().ok(), "connecting upstream");

    let client_session = random_session() & 0x7FFF;
    let hello = build_client_handshake(client_session);
    sock.send(&hello).await?;

    let mut buf = [0u8; 2048];
    let (n, _) = tokio::time::timeout(Duration::from_secs(3), sock.recv_from(&mut buf))
        .await
        .context("handshake timeout")??;
    let (hdr, payload) = decode_packet(&buf[..n])?;
    if !is_handshake_packet(hdr.flags) {
        bail!("expected handshake reply, got flags={:?}", hdr.flags);
    }
    if parse_handshake_status(payload) != Some(HANDSHAKE_OK_STATUS) {
        bail!(
            "ATEM rejected handshake: status={:?}",
            parse_handshake_status(payload)
        );
    }

    // Match atem-connection / real ATEM: ACK the HELLO packet id, then expect the next
    // id for the init dump (first reliable data is usually 1, not 0).
    let mut session_id = hdr.session_id;
    let mut rel = ReliableEndpoint::new(session_id, ReliableConfig::default());
    rel.ack.highest_recv = Some(hdr.packet_id);
    rel.ack.expected_recv = next_packet_id(hdr.packet_id);
    rel.next_send_id = 1;
    info!(
        session = format!("0x{session_id:04x}"),
        hello_pkt = hdr.packet_id,
        expect = rel.ack.expected_recv,
        "upstream handshake ok"
    );
    if let Some(ack) = rel.build_ack_packet() {
        sock.send(&ack).await?;
    }

    let mut init_complete = false;
    let mut pending_cmds: Vec<Vec<u8>> = Vec::new();
    let mut tick = tokio::time::interval(Duration::from_millis(5));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = cancel.cancelled() => return Ok(()),
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(raw) if raw.len() >= 8 => pending_cmds.push(raw),
                    Some(_) => {}
                    None => return Ok(()),
                }
            }
            res = sock.recv(&mut buf) => {
                let n = res?;
                rel.note_recv();
                let (hdr, payload) = match decode_packet(&buf[..n]) {
                    Ok(v) => v,
                    Err(e) => {
                        warn!(error = %e, "bad upstream packet");
                        continue;
                    }
                };
                if hdr.session_id != 0 {
                    session_id = hdr.session_id;
                    rel.session_id = session_id;
                }
                if hdr.flags.contains(PacketFlags::ACK_REPLY) || hdr.ack_id != 0 {
                    rel.on_ack_id(hdr.ack_id);
                }
                if is_handshake_packet(hdr.flags) {
                    continue;
                }
                let reliable = hdr.flags.contains(PacketFlags::ACK_REQUEST);
                if reliable {
                    let ok = rel.accept_reliable(
                        hdr.packet_id,
                        hdr.flags.contains(PacketFlags::IS_RETRANSMIT),
                    );
                    if !ok {
                        let req = rel.build_retransmit_request();
                        let _ = sock.send(&req).await;
                        continue;
                    }
                }
                if !payload.is_empty() {
                    handle_upstream_payload(
                        payload,
                        cache,
                        locks,
                        events,
                        &mut init_complete,
                        connected,
                        reached_ready,
                        &mut rel,
                        &sock,
                    ).await?;
                } else if reliable && !init_complete {
                    debug!("empty reliable upstream packet during init");
                }
            }
            _ = tick.tick() => {
                if rel.is_timed_out() {
                    bail!("upstream idle timeout");
                }
                if !pending_cmds.is_empty() {
                    let batch = std::mem::take(&mut pending_cmds);
                    rel.queue_commands_packed(&batch);
                }
                let now = Instant::now();
                for pkt in rel.poll_outbound(now) {
                    sock.send(&pkt).await?;
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_upstream_payload(
    payload: &[u8],
    cache: &StateCache,
    locks: &LockBroker,
    events: &broadcast::Sender<UpstreamEvent>,
    init_complete: &mut bool,
    connected: &AtomicBool,
    reached_ready: &AtomicBool,
    rel: &mut ReliableEndpoint,
    sock: &UdpSocket,
) -> Result<()> {
    let cmds = match parse_commands(payload) {
        Ok(c) => c,
        Err(e) => {
            warn!(error = %e, "upstream command parse failed; forwarding raw");
            cache.ingest_payload(payload);
            let _ = events.send(UpstreamEvent::StatePayload(Bytes::copy_from_slice(payload)));
            return Ok(());
        }
    };

    let mut state_chunk = Vec::new();
    let mut audio_chunk = Vec::new();
    let mut transfer_chunk = Vec::new();
    let mut saw_incm = false;
    for cmd in &cmds {
        if cmd.name == INIT_COMPLETE {
            saw_incm = true;
            continue;
        }
        if is_transfer_command(cmd.name) {
            transfer_chunk.extend_from_slice(cmd.raw);
            continue;
        }
        if is_lock_status(cmd.name) {
            state_chunk.extend_from_slice(cmd.raw);
            continue;
        }
        if is_ephemeral_command(cmd.name) {
            audio_chunk.extend_from_slice(cmd.raw);
        } else {
            state_chunk.extend_from_slice(cmd.raw);
        }
    }

    if !state_chunk.is_empty() {
        locks.on_upstream_lock_status(&state_chunk);
        cache.ingest_payload(&state_chunk);
        let _ = events.send(UpstreamEvent::StatePayload(Bytes::from(state_chunk)));
    }
    if !audio_chunk.is_empty() {
        let _ = events.send(UpstreamEvent::AudioLevels(Bytes::from(audio_chunk)));
    }
    if !transfer_chunk.is_empty() {
        let _ = events.send(UpstreamEvent::TransferPayload(Bytes::from(transfer_chunk)));
    }

    // Only accept clients after a real InCm — never the "10 commands" heuristic.
    if saw_incm {
        *init_complete = true;
        cache.set_ready(true);
        reached_ready.store(true, Ordering::SeqCst);
        if !connected.swap(true, Ordering::SeqCst) {
            info!(version = ?cache.version(), product = ?cache.product(), "upstream ready");
            let _ = events.send(UpstreamEvent::Connected);
            rel.opened = true;
            let ack = build_init_complete_ack(rel.session_id, rel.ack.highest_recv.unwrap_or(0));
            sock.send(&ack).await?;
        }
    }
    Ok(())
}

fn random_session() -> u16 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::time::Instant;
    let mut h = DefaultHasher::new();
    Instant::now().hash(&mut h);
    std::process::id().hash(&mut h);
    std::thread::current().id().hash(&mut h);
    h.finish() as u16
}
