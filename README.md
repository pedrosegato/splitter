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

```
cargo run -p audiomirror-cli -- loop --input ignored --output <output id> --source system
cargo run -p audiomirror-cli -- send --input ignored --addr <ip>:5004 --source system
```

**macOS:** On first run AudioMirror asks for **Screen Recording** permission. Approve it in System Settings → Privacy & Security → Screen Recording, then relaunch. Requires macOS 13 or newer.

**Windows:** WASAPI loopback runs automatically; no extra setup.

**Linux:** Any PulseAudio or PipeWire `.monitor` source is picked up automatically.
