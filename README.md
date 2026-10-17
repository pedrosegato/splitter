# Splitter

Send audio from one computer to another on the same network. Pick a source on
one machine (a microphone or whatever the system is playing) and an output on
the other. No server, no account.

Built with heavy AI assistance. The point was to put what I know about LAN and
networking into practice. I don't actually know Rust or Tauri, I picked them for
the performance.

## Download

Grab the installer for your OS from
[Releases](https://github.com/pedrosegato/splitter/releases/latest) (macOS,
Windows, Linux).

Builds aren't code-signed yet, so the first launch shows a warning:

- **macOS:** right-click the app and choose Open, or run
  `xattr -dr com.apple.quarantine /Applications/Splitter.app`.
- **Windows:** in the SmartScreen dialog, click More info, then Run anyway.

## Usage

1. Open Splitter on both machines. They find each other on the LAN.
2. Click the other machine and check that both screens show the same six-digit
   code. You only do this once per pair.
3. Drag a source onto an output to start a stream.

Windows and Linux can capture system audio directly. On macOS, install
[BlackHole 2ch](https://existential.audio/blackhole/) and route the system
output through it. To use received audio as a microphone in another app, send
it to BlackHole (macOS) or [VB-Cable](https://vb-audio.com/Cable/) (Windows).

## How it works

Audio is captured in 20 ms frames at 48 kHz, encoded with Opus and sent over
UDP. The receiver plays it through an adaptive jitter buffer. Peers are found
via mDNS and negotiate streams over TCP. Pairing is an X25519 exchange checked
by the six-digit code. Signaling and audio are encrypted with
ChaCha20-Poly1305.

## Build

Needs Rust, Node 20, libopus and the
[Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for your OS.

```sh
npm ci
npm run tauri build
```

`crates/splitter-cli` runs the same engine without the GUI.
