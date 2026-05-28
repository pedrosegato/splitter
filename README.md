# AudioMirror

Lightweight LAN audio mirror between PCs.

## Build

```
cargo build --workspace --release
```

## Try the CLI (Phase 1)

Subcommands land in Task 13 onward; the current scaffold only verifies the binary runs.

```
cargo run -p audiomirror-cli -- devices
cargo run -p audiomirror-cli -- loop --input <id> --output <id>
```

## System audio (desktop mirror)

AudioMirror can mirror your desktop audio (not just a microphone) between two computers.

### macOS (BlackHole required)

ScreenCaptureKit is too limited (mono only, no per-channel separation, extra latency). The reliable path is the [BlackHole](https://existential.audio/blackhole/) virtual audio driver.

1. Install BlackHole 2ch: `brew install --cask blackhole-2ch`
2. **System Settings → Sound → Output → BlackHole 2ch.** Everything the system plays now routes to BlackHole instead of your speakers/headset.
3. Run with BlackHole as the input and your real device as the output:

```
cargo run -p audiomirror-cli -- loop --input "BlackHole 2ch" --output "<your headset or speaker name>"
```

You can list available devices first with `cargo run -p audiomirror-cli -- devices`.

If you want to keep hearing the desktop audio locally AND capture it, create a **Multi-Output Device** in `/Applications/Utilities/Audio MIDI Setup.app` containing BlackHole 2ch + your real output, then set the system Output to that Multi-Output. Do NOT also use that Multi-Output as the app's `--output` — it creates a feedback loop with BlackHole.

The legacy `--source system` flag still exists and uses ScreenCaptureKit; expect mono and worse latency.

### Windows

WASAPI loopback runs automatically:

```
cargo run -p audiomirror-cli -- loop --input ignored --output <output id> --source system
```

### Linux

Any PulseAudio or PipeWire `.monitor` source is picked up automatically when you use `--source system`.

## P2P discovery and signaling (Phase 2)

Run the long-running daemon on each machine:

```
cargo run -p audiomirror-cli -- daemon --signaling-port 7000
```

On first launch a UUID identity is written to your OS config dir (`~/Library/Application Support/AudioMirror/identity.toml` on macOS). The daemon registers `_audiomirror._tcp.local.` on mDNS and listens on TCP for incoming signaling connections.

Interactive stdin commands:

| Command | Effect |
| --- | --- |
| `peers` | Print currently discovered peers (mDNS). |
| `pending` | Print queued HELLO messages awaiting trust acceptance. |
| `accept <idx>` | Promote a queued HELLO to trusted; generates and stores an auth token. |
| `connect <peer_id\|name>` | Open a TCP signaling link to a peer found via `peers`. |
| `open <peer_id\|name>` | Open a Session with an already-connected peer. |
| `sessions` | List active sessions and their streams. |
| `disconnect <session_id>` | Close a session and all its streams. |
| `quit` | Shut the daemon down. |

For a one-shot mDNS browse without persistent state:

```
cargo run -p audiomirror-cli -- discover --duration-secs 5
```

**Trust model (MVP):** TOFU. The first time a peer connects, its HELLO sits in `pending` until the local operator accepts. After acceptance, the auth token is persisted in `trusted_peers.toml`; reconnects are silent if the token still matches.

## Modular routing (Phase 3)

Within a daemon REPL, after `open <peer>` has produced a session:

| Command | Effect |
| --- | --- |
| `stream open --session <session_id> --from <local-device-id-or "system"> --to <peer>:<remote-device-id-or "default"> [--bitrate N]` | Initiator opens a UDP stream. The other side replies with the bound sink port and the source pump starts. |
| `stream close <session_id>:<stream_id>` | Tear down a stream end-to-end (sends StreamControl close). |
| `stream volume <session_id>:<stream_id> <0-100>` | Set linear gain on the local pump and tell the peer. |
| `stream mute <session_id>:<stream_id>` / `stream unmute …` | Zero gain (packets keep flowing so loss stats remain accurate). |
| `stream pause <session_id>:<stream_id>` / `stream resume …` | Suspend the pump and signal the peer. |
| `stream stats [<session_id>:<stream_id>]` | Tabular live view of packets sent/recv/lost, kbps, RTT. Refreshes every 1s, exits on Ctrl-C. |

Auto-pause: if a bound device disappears (USB unplug, headset off), the relevant pump pauses and signals the peer. When the device returns, the pump resumes automatically.

## Settings & ops (Phase 4)

AudioMirror reads a single TOML at `~/Library/Application Support/AudioMirror/settings.toml` (macOS) / `~/.config/AudioMirror/settings.toml` (Linux) / `%APPDATA%\AudioMirror\settings.toml` (Windows).

```bash
audiomirror-cli settings show
audiomirror-cli settings set fec_mode auto
audiomirror-cli settings set jitter_mode fixed:40
audiomirror-cli settings set metrics_enabled true
```

**Adaptive FEC** is on by default in `auto` mode: above 1% measured loss → inband FEC turns on; below 0.2% → off; 10s hysteresis. Force on/off with `settings set fec_mode always|never`.

**Adaptive jitter buffer** in `auto` mode targets P99 arrival jitter; force a fixed depth with `settings set jitter_mode fixed:<ms>`; minimum with `min`.

**Auto-start with system:**

```bash
audiomirror-cli autostart enable    # writes launchd / systemd / Windows Run entry
audiomirror-cli autostart status
audiomirror-cli autostart disable
```

**Structured logs:** rotated daily, 7-day retention. View live: `audiomirror-cli logs tail`. Path: `audiomirror-cli logs path`.

**Prometheus metrics (opt-in):**

```bash
audiomirror-cli metrics enable
audiomirror-cli daemon &
curl http://localhost:9000/metrics
```

**Auto-accept trusted peers:** once a peer is TOFU-trusted, set `settings set auto_accept_trusted true` to skip the manual `accept <n>` step on reconnect.
