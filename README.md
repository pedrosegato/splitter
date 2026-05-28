# Splitter

Lightweight LAN audio mirror — stream desktop or microphone audio between two
machines over UDP with Opus encoding, adaptive jitter buffer, and TOFU peer
trust. No cloud. No account. Sub-200 ms end-to-end latency on a typical LAN.

## Quick start (Mac to Windows)

**1. Build on both machines.**

```sh
cargo build --workspace --release
# Or use the launcher scripts (no Rust toolchain needed after building once):
#   scripts/run-daemon.sh
#   scripts/run-daemon.bat
```

**2. Start the daemon on each machine.**

Mac:
```sh
scripts/run-daemon.sh alice
# Prints: READY port=7000
```

Windows:
```bat
scripts\run-daemon.bat bob
```

**3. Connect and pair.**

On Alice (Mac), type into the REPL:
```
connect 192.168.1.50:7000   # or: connect bob  (if mDNS resolves it)
```

On Bob (Windows), when the `>> pending: alice` notification appears:
```
accept 0
```

Alice then opens a session:
```
open bob
```

**4. Start a stream.**

On Alice, after `open` prints a session UUID:
```
stream open --from in:0 --to bob:out:1 --session <UUID>
```

This captures Alice's BlackHole 2ch input (see macOS setup below) and plays it
on Bob's default output device.

---

## Per-platform setup

### macOS

ScreenCaptureKit is mono-only with extra latency. Use
[BlackHole 2ch](https://existential.audio/blackhole/) as a virtual loopback:

```sh
brew install --cask blackhole-2ch
```

In **System Settings → Sound → Output**, select **BlackHole 2ch**. Everything
the system plays now routes through BlackHole. To hear it locally too, create a
**Multi-Output Device** in `/Applications/Utilities/Audio MIDI Setup.app`
combining BlackHole 2ch and your real output, then set System Output to that
Multi-Output Device.

List available device indices:
```sh
splitter-cli devices
```

Pass the BlackHole index as `--from in:<idx>` in `stream open`.

### Windows

WASAPI loopback is selected automatically — no extra software needed. The
`--source system` flag on `send` / `loop` enables it.

### Linux

Any PulseAudio or PipeWire `.monitor` source is available automatically. List
sources with `splitter-cli devices` and pass the `.monitor` source index to
`--from`.

---

## Daemon REPL command reference

Start the daemon:
```sh
splitter-cli daemon [--signaling-port 7000] [--peer-name "Studio Mac"] [--identity-dir <path>]
```

After `READY port=<N>`, the daemon accepts line commands on stdin:

| Command | Description |
|---------|-------------|
| `peers` | List mDNS-discovered peers (index, name, peer-id, host:port) |
| `pending` | List peers whose Hello is awaiting trust acceptance |
| `accept <n>` | Accept the pending peer at index `<n>` and persist the auth token |
| `connect <name\|id\|host:port>` | Open a TCP signaling link to a peer |
| `open <name\|id>` | Open a new Session with a connected peer |
| `sessions` | List active sessions with state and per-stream status |
| `stream open --from <dev> --to <peer>:<dev> --session <UUID> [--bitrate N]` | Start an audio stream |
| `stream close <session>:<stream>` | Tear down a stream end-to-end |
| `stream volume <session>:<stream> <0-100>` | Set playback gain |
| `stream mute / unmute <session>:<stream>` | Mute/unmute (packets keep flowing) |
| `stream pause / resume <session>:<stream>` | Pause/resume pump |
| `stream stats [<session>:<stream>]` | Live statistics table (Ctrl-C to stop) |
| `disconnect <session_id>` | Close a session and all its streams |
| `settings show / get <key> / set <key> <value>` | Inspect or update settings |
| `logs path / logs tail` | Print log path or follow log output |
| `autostart enable / disable / status` | Manage system autostart |
| `metrics enable / disable / status` | Manage Prometheus endpoint |
| `quit` | Graceful shutdown |

---

## Settings & ops

See [CLI-REFERENCE.md](CLI-REFERENCE.md) for the full flag reference, all
settings keys, and exit codes.

Quick examples:
```sh
splitter-cli settings show
splitter-cli settings set fec_mode always
splitter-cli settings set jitter_mode min
splitter-cli settings set log_level debug

splitter-cli autostart enable     # launchd / systemd / Windows Run
splitter-cli logs tail            # follow the log
splitter-cli metrics enable       # then curl http://localhost:9000/metrics
```

---

## How it works

Splitter is a two-process peer-to-peer system. Each machine runs a daemon
that handles signaling (TCP), discovery (mDNS), and audio transport (UDP).
Audio is captured in 20 ms stereo frames at 48 kHz, encoded with Opus at
64 kbps, and transmitted over UDP to the remote peer, which decodes and plays
the frames through an adaptive jitter buffer.

```
Mac                  LAN                Windows
─────                ───                ───────
System audio
  |
BlackHole 2ch
  |
[capture]
  |
Opus encoder ── UDP ──> Opus decoder
                              |
                         [playback]
                              |
                         Audio output
```

Trust is established on first connection via TOFU: the receiver sees a `pending`
entry, the operator runs `accept <n>`, and an auth token is persisted so future
reconnects are automatic (with `auto_accept_trusted = true` in settings).

---

## Limitations

- macOS ScreenCaptureKit capture is mono only; use BlackHole 2ch for full stereo.
- Multi-peer sessions (more than two participants) are not yet supported (Phase 9).
- The `loop` subcommand's `--source system` flag works on Windows (WASAPI) and
  Linux (.monitor); on macOS use BlackHole 2ch as `--input` instead.

---

## Build

```sh
cargo build --workspace --release
```

```sh
# Individual subcommands (development)
cargo run -p splitter-cli -- devices
cargo run -p splitter-cli -- daemon --signaling-port 7000
```
