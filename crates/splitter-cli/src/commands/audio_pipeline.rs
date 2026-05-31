use anyhow::Result;
use socket2::{Domain, Protocol, SockAddr, Socket, Type};
use splitter_core::audio::capture::CaptureHandle;
use splitter_core::audio::ring::RingProducer;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;
use tokio::net::UdpSocket;
use tokio::sync::Notify;

#[cfg(all(target_os = "macos", feature = "sck"))]
use splitter_core::MacosLoopbackHandle;

pub(crate) const FEC_REEVAL_FRAMES: u32 = 100;

#[allow(dead_code)]
pub(crate) enum CaptureGuard {
    Mic(CaptureHandle),
    #[cfg(all(target_os = "macos", feature = "sck"))]
    MacSystem(MacosLoopbackHandle),
    #[cfg(not(target_os = "macos"))]
    Loopback(CaptureHandle),
}

impl CaptureGuard {
    pub(crate) fn frame_ready(&self) -> Arc<Notify> {
        match self {
            CaptureGuard::Mic(h) => h.frame_ready(),
            #[cfg(all(target_os = "macos", feature = "sck"))]
            CaptureGuard::MacSystem(h) => h.frame_ready(),
            #[cfg(not(target_os = "macos"))]
            CaptureGuard::Loopback(h) => h.frame_ready(),
        }
    }
}

pub(crate) fn start_capture(
    source: crate::Source,
    input: &str,
    producer: RingProducer,
) -> Result<CaptureGuard> {
    match source {
        crate::Source::Mic => Ok(CaptureGuard::Mic(CaptureHandle::start(input, producer)?)),
        crate::Source::System => {
            #[cfg(all(target_os = "macos", feature = "sck"))]
            {
                Ok(CaptureGuard::MacSystem(
                    splitter_core::MacosLoopbackHandle::start(producer)?,
                ))
            }
            #[cfg(all(target_os = "macos", not(feature = "sck")))]
            {
                let _ = producer;
                anyhow::bail!("system audio capture requires the sck feature (use BlackHole 2ch as an input device instead)");
            }
            #[cfg(not(target_os = "macos"))]
            {
                Ok(CaptureGuard::Loopback(CaptureHandle::start_loopback(
                    producer,
                )?))
            }
        }
    }
}

pub(crate) fn map_fec_mode(m: crate::SendFecMode) -> splitter_core::FecMode {
    match m {
        crate::SendFecMode::Auto => splitter_core::FecMode::Auto,
        crate::SendFecMode::Always => splitter_core::FecMode::Always,
        crate::SendFecMode::Never => splitter_core::FecMode::Never,
    }
}

pub(crate) fn reeval_fec(
    fec: &mut splitter_core::net::fec::FecController,
    encoder: &mut splitter_core::audio::codec::OpusEncoder,
    simulated_loss_pct: u8,
) -> Result<()> {
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
    Ok(())
}

pub(crate) enum UdpDirection {
    Send,
    Recv,
}

pub(crate) fn make_udp_socket(bind: SocketAddr, direction: UdpDirection) -> Result<UdpSocket> {
    let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    match direction {
        UdpDirection::Send => sock.set_send_buffer_size(1 << 20)?,
        UdpDirection::Recv => sock.set_recv_buffer_size(1 << 20)?,
    }
    sock.bind(&SockAddr::from(bind))?;
    sock.set_nonblocking(true)?;
    let std_sock: std::net::UdpSocket = sock.into();
    Ok(UdpSocket::from_std(std_sock)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_fec_mode_auto() {
        assert!(matches!(
            map_fec_mode(crate::SendFecMode::Auto),
            splitter_core::FecMode::Auto
        ));
    }

    #[test]
    fn map_fec_mode_always() {
        assert!(matches!(
            map_fec_mode(crate::SendFecMode::Always),
            splitter_core::FecMode::Always
        ));
    }

    #[test]
    fn map_fec_mode_never() {
        assert!(matches!(
            map_fec_mode(crate::SendFecMode::Never),
            splitter_core::FecMode::Never
        ));
    }

    #[tokio::test]
    async fn make_udp_socket_send_binds_ephemeral_port() {
        let bind = "0.0.0.0:0".parse().unwrap();
        let sock = make_udp_socket(bind, UdpDirection::Send);
        assert!(sock.is_ok(), "make_udp_socket(Send) must bind successfully");
    }

    #[tokio::test]
    async fn make_udp_socket_recv_binds_ephemeral_port() {
        let bind = "0.0.0.0:0".parse().unwrap();
        let sock = make_udp_socket(bind, UdpDirection::Recv);
        assert!(sock.is_ok(), "make_udp_socket(Recv) must bind successfully");
    }
}
