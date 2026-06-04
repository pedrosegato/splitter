# Splitter

Lightweight LAN audio mirror — stream desktop or microphone audio between two
machines over UDP with Opus encoding, adaptive jitter buffer, and TOFU peer
trust. No cloud. No account. Sub-200 ms end-to-end latency on a typical LAN.

---

## A note on how this was built

This project was built fast, with heavy AI assistance. I reviewed the code, did
the security-sensitive parts by hand, and I stand behind the architecture. My
own depth is in the product vision and the networking / peer model — more than
in Rust or Tauri specifically. Audits and PRs are very welcome.

---

## Desktop app

### Download

Download the latest release from
[GitHub Releases](https://github.com/pedrosegato/splitter/releases/latest)
and pick the asset for your OS:

| OS | Asset |
|----|-------|
| macOS Apple Silicon | `Splitter_x.y.z_aarch64.dmg` |
| macOS Intel | `Splitter_x.y.z_x64.dmg` |
| Windows | `Splitter_x.y.z_x64-setup.exe` or `Splitter_x.y.z_x64_en-US.msi` |
| Linux (any) | `splitter_x.y.z_amd64.AppImage` or `splitter_x.y.z_amd64.deb` |

### Unsigned-build caveat

Current builds are **unsigned** — code-signing is deferred to a later phase.
This triggers OS security warnings on first launch:

- **macOS** — Gatekeeper will say the app is from an unidentified developer.
  Right-click the `.dmg` → **Open**, then confirm. Alternatively, remove the
  quarantine attribute after mounting:
  ```sh
  xattr -dr com.apple.quarantine /Applications/Splitter.app
  ```
- **Windows** — SmartScreen will show a blue warning. Click **More info** →
  **Run anyway**.

These warnings disappear once Apple and Windows code-signing are provisioned
(upcoming release phase).

### Prerequisites — system audio and virtual mic

To capture what the system plays (not just a microphone), you need a virtual
audio loopback driver:

| OS | What to install |
|----|-----------------|
| macOS | [BlackHole 2ch](https://existential.audio/blackhole/) (`brew install --cask blackhole-2ch`) |
| Windows | [VB-Cable](https://vb-audio.com/Cable/) (free download, run installer as Administrator) |
| Linux | Nothing — PulseAudio/PipeWire `.monitor` sources are available automatically |

### Quick usage

1. Open **Splitter** on both machines — both must be on the same LAN.
2. Each machine advertises itself via mDNS. Peers appear in the sidebar
   automatically within a few seconds.
3. Click a peer to open a session. On first connection the remote side sees a
   trust prompt; accept it to persist the pairing.
4. In the routing canvas, drag a **source port** (e.g., BlackHole 2ch on the
   sender) to a **destination port** (e.g., Default Output on the receiver) to
   start a stream.
5. Adjust volume, mute, or pause streams from the stream row controls.

### Auto-update

The app checks GitHub Releases for new versions on launch. You can also trigger
a manual check from **Settings → About → Check for updates**. Updates are
downloaded in the background and applied on next launch.

---

## CLI

### Quick start (Mac to Windows)

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

Run `splitter-cli --help` (and `splitter-cli <command> --help`) for the full
flag reference, all settings keys, and exit codes.

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
