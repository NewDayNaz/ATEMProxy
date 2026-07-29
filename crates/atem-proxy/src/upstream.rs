use crate::cache::StateCache;
use anyhow::{bail, Context, Result};
use atem_protocol::{
    build_client_handshake, build_init_complete_ack, decode_packet, is_ephemeral_command,
    is_handshake_packet, parse_commands, parse_handshake_status, PacketFlags, ReliableConfig,
    ReliableEndpoint, HANDSHAKE_OK_STATUS, INIT_COMPLETE,
};
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
    StatePayload(Vec<u8>),
    /// High-rate audio levels for subscribers only.
    AudioLevels(Vec<u8>),
}

pub struct UpstreamHandle {
    pub connected: Arc<AtomicBool>,
    pub cmd_tx: mpsc::Sender<Vec<u8>>,
    pub events: broadcast::Sender<UpstreamEvent>,
}

pub fn spawn_upstream(
    atem: SocketAddr,
    cache: Arc<StateCache>,
    cancel: CancellationToken,
    reconnect_ms: u64,
) -> UpstreamHandle {
    let connected = Arc::new(AtomicBool::new(false));
    let (cmd_tx, mut cmd_rx) = mpsc::channel::<Vec<u8>>(512);
    let (events, _) = broadcast::channel(512);
    let connected_flag = connected.clone();
    let events_tx = events.clone();

    tokio::spawn(async move {
        let mut backoff = Duration::from_millis(reconnect_ms.max(200));
        while !cancel.is_cancelled() {
            match run_session(
                atem,
                &cache,
                &connected_flag,
                &events_tx,
                &mut cmd_rx,
                &cancel,
            )
            .await
            {
                Ok(()) => info!("upstream session closed"),
                Err(e) => error!(error = %e, "upstream session failed"),
            }
            connected_flag.store(false, Ordering::SeqCst);
            cache.clear();
            let _ = events_tx.send(UpstreamEvent::Disconnected);
            if cancel.is_cancelled() {
                break;
            }
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = tokio::time::sleep(backoff) => {}
            }
            backoff = (backoff * 2).min(Duration::from_secs(30));
        }
    });

    UpstreamHandle {
        connected,
        cmd_tx,
        events,
    }
}

async fn run_session(
    atem: SocketAddr,
    cache: &StateCache,
    connected: &AtomicBool,
    events: &broadcast::Sender<UpstreamEvent>,
    cmd_rx: &mut mpsc::Receiver<Vec<u8>>,
    cancel: &CancellationToken,
) -> Result<()> {
    let sock = UdpSocket::bind("0.0.0.0:0")
        .await
        .context("bind upstream UDP socket")?;
    sock.connect(atem)
        .await
        .with_context(|| format!("connect to ATEM {atem}"))?;
    info!(%atem, local = ?sock.local_addr().ok(), "connecting upstream");

    let client_session = (rand_session()) & 0x7FFF;
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
        bail!("ATEM rejected handshake: status={:?}", parse_handshake_status(payload));
    }

    // Session ID may still be temporary; adopt from subsequent packets.
    let mut session_id = hdr.session_id;
    let mut rel = ReliableEndpoint::new(session_id, ReliableConfig::default());
    // ACK packet 0 / open
    if let Some(ack) = {
        rel.ack.highest_recv = Some(0);
        rel.build_ack_packet()
    } {
        sock.send(&ack).await?;
    }

    let mut init_complete = false;
    let mut tick = tokio::time::interval(Duration::from_millis(5));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = cancel.cancelled() => return Ok(()),
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(raw) => {
                        // Pack single framed command(s)
                        if raw.len() >= 8 {
                            rel.queue_payload(raw);
                        }
                    }
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
                        events,
                        &mut init_complete,
                        connected,
                        &mut rel,
                        &sock,
                    ).await?;
                } else if reliable && !init_complete {
                    // Empty reliable packet often marks end of init flood.
                    debug!("empty reliable upstream packet during init");
                }
            }
            _ = tick.tick() => {
                if rel.is_timed_out() {
                    bail!("upstream idle timeout");
                }
                let now = Instant::now();
                for pkt in rel.poll_outbound(now) {
                    sock.send(&pkt).await?;
                }
            }
        }
    }
}

async fn handle_upstream_payload(
    payload: &[u8],
    cache: &StateCache,
    events: &broadcast::Sender<UpstreamEvent>,
    init_complete: &mut bool,
    connected: &AtomicBool,
    rel: &mut ReliableEndpoint,
    sock: &UdpSocket,
) -> Result<()> {
    let cmds = match parse_commands(payload) {
        Ok(c) => c,
        Err(e) => {
            warn!(error = %e, "upstream command parse failed; forwarding raw");
            cache.ingest_payload(payload);
            let _ = events.send(UpstreamEvent::StatePayload(payload.to_vec()));
            return Ok(());
        }
    };

    let mut state_chunk = Vec::new();
    let mut audio_chunk = Vec::new();
    for cmd in &cmds {
        if cmd.name == INIT_COMPLETE {
            *init_complete = true;
            cache.ingest_payload(payload);
            cache.set_ready(true);
            if !connected.swap(true, Ordering::SeqCst) {
                info!(version = ?cache.version(), product = ?cache.product(), "upstream ready");
                let _ = events.send(UpstreamEvent::Connected);
                rel.opened = true;
                let ack = build_init_complete_ack(rel.session_id, rel.ack.highest_recv.unwrap_or(0));
                sock.send(&ack).await?;
            }
            continue;
        }
        if is_ephemeral_command(cmd.name) {
            audio_chunk.extend_from_slice(cmd.raw);
        } else {
            state_chunk.extend_from_slice(cmd.raw);
        }
    }

    if !state_chunk.is_empty() {
        cache.ingest_payload(&state_chunk);
        let _ = events.send(UpstreamEvent::StatePayload(state_chunk));
    }
    if !audio_chunk.is_empty() {
        let _ = events.send(UpstreamEvent::AudioLevels(audio_chunk));
    }

    // Mark ready once we've seen substantial state even without InCm (some firmwares).
    if !*init_complete && cache.len() > 10 && !connected.load(Ordering::SeqCst) {
        cache.set_ready(true);
        connected.store(true, Ordering::SeqCst);
        rel.opened = true;
        info!("upstream ready (heuristic, no InCm yet)");
        let _ = events.send(UpstreamEvent::Connected);
    }
    Ok(())
}

fn rand_session() -> u16 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u16)
        .unwrap_or(0x1234);
    t ^ std::process::id() as u16
}
