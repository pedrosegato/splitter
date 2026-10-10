# Splitter

Lightweight LAN audio mirror — stream desktop or microphone audio between two
machines over UDP with Opus encoding, adaptive jitter buffer, and encrypted,
code-verified pairing. No cloud. No account. Sub-200 ms end-to-end latency on a
typical LAN.

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

| OS                  | Asset                                                            |
| ------------------- | ---------------------------------------------------------------- |
| macOS Apple Silicon | `Splitter_x.y.z_aarch64.dmg`                                     |
| macOS Intel         | `Splitter_x.y.z_x64.dmg`                                         |
| Windows             | `Splitter_x.y.z_x64-setup.exe` or `Splitter_x.y.z_x64_en-US.msi` |
| Linux (any)         | `splitter_x.y.z_amd64.AppImage` or `splitter_x.y.z_amd64.deb`    |

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

| OS      | What to install                                                                             |
| ------- | ------------------------------------------------------------------------------------------- |
| macOS   | [BlackHole 2ch](https://existential.audio/blackhole/) (`brew install --cask blackhole-2ch`) |
| Windows | [VB-Cable](https://vb-audio.com/Cable/) (free download, run installer as Administrator)     |
| Linux   | Nothing — PulseAudio/PipeWire `.monitor` sources are available automatically                |

### Quick usage

1. Open **Splitter** on both machines — both must be on the same LAN.
2. Each machine advertises itself via mDNS. Peers appear in the sidebar
   automatically within a few seconds.
3. Click a peer to open a session. On first connection both machines show the
   same six-digit code; the remote side accepts only if the codes match, and the
   pairing is remembered.
4. In the routing canvas, drag a **source port** (e.g., BlackHole 2ch on the
   sender) to a **destination port** (e.g., Default Output on the receiver) to
   start a stream.
5. Adjust volume, mute, or pause streams from the stream row controls.

### Auto-update

The app checks GitHub Releases for new versions on launch. You can also trigger
a manual check from **Settings → About → Check for updates**. Updates are
downloaded in the background and applied on next launch.

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

Pairing uses an ephemeral X25519 exchange: both sides derive the pairing token
from it (it never crosses the network) along with a six-digit code shown on both
screens, so a man-in-the-middle shows up as mismatched codes. The receiver sees a
`pending` entry, the operator compares the code and accepts (`accept <n>` in the
CLI), and the token is stored on both machines. Later connections prove the
token with HMACs over the handshake and reconnect automatically (with
`auto_accept_trusted = true` in settings). Every connection derives fresh keys:
signaling, audio and receiver reports are encrypted and authenticated with
ChaCha20-Poly1305, and the receiver drops anything that fails authentication.

---

## Limitations

- macOS ScreenCaptureKit capture is mono only; use BlackHole 2ch for full stereo.
- Multi-peer sessions (more than two participants) are not yet supported (Phase 9).
- The `loop` subcommand's `--source system` flag works on Windows (WASAPI) and
  Linux (.monitor); on macOS use BlackHole 2ch as `--input` instead.

---

## Build

### System dependencies

The workspace links against libopus (all platforms) plus, on Linux, the Tauri
GUI/audio stack. Install them before building:

**macOS**

```sh
brew install opus pkg-config
```

**Linux (Debian/Ubuntu)**

```sh
sudo apt-get install -y libwebkit2gtk-4.1-dev libgtk-3-dev \
  libayatana-appindicator3-dev librsvg2-dev libasound2-dev \
  libudev-dev libopus-dev pkg-config
```

**Windows** (via [vcpkg](https://vcpkg.io))

```sh
vcpkg install opus:x64-windows
```

### Compile

```sh
cargo build --workspace --release
```

```sh
# Individual subcommands (development)
cargo run -p splitter-cli -- devices
cargo run -p splitter-cli -- daemon --signaling-port 7000
```
