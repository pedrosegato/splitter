# Changelog

All notable changes to AudioMirror are documented here.

## [v0.1.0-cli] — unreleased

### Added
- Cross-platform audio mirror CLI: macOS (BlackHole 2ch), Windows (WASAPI loopback), Linux (.monitor sources)
- TOFU peer trust with persistent identity and token exchange in HelloAck
- mDNS peer discovery on `_audiomirror._tcp.local.`
- Two-machine bidirectional sessions with multiple concurrent streams per session
- Adaptive jitter buffer (200ms target) and FEC always-on by default
- Opus codec at 64 kbps stereo, 20 ms frames, 48 kHz
- Daemon REPL with peers / pending / accept / connect / open / sessions / stream open|close|volume|mute|pause|resume|stats / disconnect / quit
- Polished ASCII-boxed listings for screen recording
- Cross-side notifications: every peer-initiated event prints `>> ...` on the receiver
- Settings persistence in `~/.audiomirror/<identity>/` with hot-reload (5s mtime poll)
- Launcher scripts (`scripts/run-daemon.sh`, `scripts/run-daemon.bat`)
- Per-stream stats: packets sent/received/lost, bitrate, RTT, process CPU%, total bandwidth
- Auto-reconnect with exponential backoff when a TCP signaling link drops
- Graceful shutdown that sends `StreamControl{Close}` + `SessionResponse{closed}` before TCP drop
- Prometheus metrics endpoint (opt-in)
- Structured logging via `tracing` with daily rotation

### Known limitations
- macOS ScreenCaptureKit capture is mono only; use BlackHole 2ch input for full stereo
- Multi-peer (N>2 in a session) not yet supported; tracked as Phase 9
