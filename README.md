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
