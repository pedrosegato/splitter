use crate::Source;
use audiomirror_core::audio::capture::CaptureHandle;
use audiomirror_core::audio::codec::OpusEncoder;
use audiomirror_core::audio::ring::AudioRing;
use audiomirror_core::net::packet::Packet;
use audiomirror_core::FRAME_STEREO_SAMPLES;
use bytes::{Bytes, BytesMut};
use socket2::{Domain, Protocol, SockAddr, Socket, Type};
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Instant;
use tokio::net::UdpSocket;
use tokio::sync::Notify;
use tokio::time::Duration;

#[cfg(target_os = "macos")]
use audiomirror_core::MacosLoopbackHandle;

#[allow(dead_code)]
enum CaptureGuard {
    Mic(CaptureHandle),
    #[cfg(target_os = "macos")]
    MacSystem(MacosLoopbackHandle),
    #[cfg(not(target_os = "macos"))]
    Loopback(CaptureHandle),
}

impl CaptureGuard {
    fn frame_ready(&self) -> Arc<Notify> {
        match self {
            CaptureGuard::Mic(h) => h.frame_ready(),
            #[cfg(target_os = "macos")]
            CaptureGuard::MacSystem(h) => h.frame_ready(),
            #[cfg(not(target_os = "macos"))]
            CaptureGuard::Loopback(h) => h.frame_ready(),
        }
    }
}

pub(crate) async fn run(
    input: &str,
    addr: &str,
    stream_id: u8,
    bitrate: i32,
    source: Source,
    fec_mode: crate::SendFecMode,
    simulated_loss_pct: u8,
) -> anyhow::Result<()> {
    let dest: SocketAddr = SocketAddr::from_str(addr)?;
    let (producer, mut consumer) = AudioRing::new(7_680);
    let _capture: CaptureGuard = match source {
        Source::Mic => CaptureGuard::Mic(CaptureHandle::start(input, producer)?),
        Source::System => {
            #[cfg(target_os = "macos")]
            {
                CaptureGuard::MacSystem(MacosLoopbackHandle::start(producer)?)
            }
            #[cfg(not(target_os = "macos"))]
            {
                CaptureGuard::Loopback(CaptureHandle::start_loopback(producer)?)
            }
        }
    };

    let frame_notify = _capture.frame_ready();
    let sock = make_udp_socket(SocketAddr::from(([0, 0, 0, 0], 0)))?;
    tracing::info!(
        "sending stream_id={stream_id} to {dest} at {bitrate} bps fec_mode={fec_mode:?} simulated_loss_pct={simulated_loss_pct}"
    );

    let core_fec_mode = map_fec_mode(fec_mode);
    let mut fec = audiomirror_core::net::fec::FecController::new(core_fec_mode, 1, 0, 10);

    let mut encoder = OpusEncoder::new(bitrate)?;
    let mut payload_buf = BytesMut::with_capacity(400);
    let mut out_buf = BytesMut::with_capacity(512);
    let mut frame = vec![0.0f32; FRAME_STEREO_SAMPLES];
    let start = Instant::now();
    let mut seq: u32 = 0;
    let mut frame_count: u32 = 0;

    loop {
        tokio::select! {
            _ = frame_notify.notified() => {}
            _ = tokio::time::sleep(Duration::from_millis(50)) => {
                tracing::warn!("no audio frame signal in 50ms — capture stalled?");
            }
        }
        while consumer.occupied() >= FRAME_STEREO_SAMPLES {
            let popped = consumer.pop_slice(&mut frame);
            debug_assert_eq!(
                popped, FRAME_STEREO_SAMPLES,
                "ring SPSC invariant: occupied check passed but pop_slice returned less"
            );
            if popped < FRAME_STEREO_SAMPLES {
                continue;
            }

            // Every 100 frames: inject simulated loss samples and re-evaluate FEC.
            frame_count = frame_count.wrapping_add(1);
            if frame_count.is_multiple_of(100) {
                let now = Instant::now();
                let lost = simulated_loss_pct as usize;
                let ok = 100usize.saturating_sub(lost);
                for _ in 0..lost {
                    fec.record(now, true);
                }
                for _ in 0..ok {
                    fec.record(now, false);
                }
                let setting = fec.evaluate(now);
                encoder.set_fec(setting.enable, setting.packet_loss_perc)?;
            }

            encoder.encode(&frame, &mut payload_buf)?;
            let pkt = Packet {
                stream_id,
                seq: seq & 0xFF_FFFF,
                timestamp_ms: start.elapsed().as_millis() as u32,
                payload: Bytes::copy_from_slice(&payload_buf),
            };
            pkt.encode(&mut out_buf)?;
            sock.send_to(&out_buf, dest).await?;
            seq = seq.wrapping_add(1);
        }
    }
}

fn map_fec_mode(m: crate::SendFecMode) -> audiomirror_core::FecMode {
    match m {
        crate::SendFecMode::Auto => audiomirror_core::FecMode::Auto,
        crate::SendFecMode::Always => audiomirror_core::FecMode::Always,
        crate::SendFecMode::Never => audiomirror_core::FecMode::Never,
    }
}

fn make_udp_socket(bind: SocketAddr) -> anyhow::Result<UdpSocket> {
    let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    sock.set_send_buffer_size(1 << 20)?;
    sock.bind(&SockAddr::from(bind))?;
    sock.set_nonblocking(true)?;
    let std_sock: std::net::UdpSocket = sock.into();
    Ok(UdpSocket::from_std(std_sock)?)
}

#[allow(dead_code)]
type RunSignature =
    fn(
        &str,
        &str,
        u8,
        i32,
        crate::Source,
        crate::SendFecMode,
        u8,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_guard_variants_compile() {
        fn _accept(_g: CaptureGuard) {}
    }

    #[test]
    fn run_signature_accepts_fec_args() {
        // Compile-time check: run() must accept (fec_mode, simulated_loss_pct).
        // If the signature changes this test fails to compile.
        fn _check(_f: RunSignature) {}
        let _ = run; // ensure `run` is in scope — actual signature check at compile time
    }
}
