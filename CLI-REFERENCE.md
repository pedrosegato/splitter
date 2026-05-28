# AudioMirror CLI Reference

Command reference for `audiomirror-cli`. All subcommands exit with **0** on success
and **non-zero** on error unless noted otherwise.

---

## Table of Contents

- [devices](#devices)
- [send](#send)
- [recv](#recv)
- [loop](#loop)
- [discover](#discover)
- [daemon](#daemon)
  - [Daemon REPL protocol](#daemon-repl-protocol)
- [stream open](#stream-open)
- [stats](#stats)
- [logs](#logs)
- [settings](#settings)
- [autostart](#autostart)
- [metrics](#metrics)

---

## devices

**Synopsis:** `audiomirror-cli devices`

**Description:**
Enumerate all audio input and output devices available on the host and print
them to stdout. Useful for obtaining the device ID strings required by `send`,
`recv`, `loop`, and `stream open`.

**Flags:** none

**Example:**

```sh
audiomirror-cli devices
```

**Exit codes:**

| Code | Meaning |
|------|---------|
| 0    | Device list printed successfully |
| 1    | Audio host initialisation error |

**See also:** [send](#send), [recv](#recv), [loop](#loop)

---

## send

**Synopsis:** `audiomirror-cli send --input <device-id> --addr <host:port> [flags]`

**Description:**
Capture audio from a local input device, encode it with Opus, and transmit UDP
packets to the specified address. Runs until interrupted (Ctrl-C). Use
`--source system` to capture system/loopback audio instead of a microphone.

**Flags:**

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--input` | string | *(required)* | Device ID from `audiomirror-cli devices` |
| `--addr` | string | *(required)* | Destination `host:port`, e.g. `192.168.1.50:5004` |
| `--stream-id` | u8 | `0` | Stream identifier embedded in UDP packet header |
| `--bitrate` | i32 | `64000` | Opus target bitrate in bits/s |
| `--source` | `mic\|system` | `mic` | Audio source type |
| `--fec-mode` | `auto\|always\|never` | `auto` | Forward-error-correction mode |
| `--simulated-loss-pct` | u8 | `0` | Inject artificial packet loss (0–100) for testing |

**Example:**

```sh
# Send mic to a remote host
audiomirror-cli send --input "Input:0:Built-in Microphone" --addr 192.168.1.50:5004

# Send system audio with FEC always on
audiomirror-cli send --input "Input:0:Built-in Microphone" --addr 10.0.0.2:5004 \
    --source system --fec-mode always --bitrate 96000
```

**Exit codes:**

| Code | Meaning |
|------|---------|
| 0    | Clean shutdown (Ctrl-C) |
| 1    | Device not found, socket error, or Opus encoder failure |

**See also:** [recv](#recv), [loop](#loop), [devices](#devices)

---

## recv

**Synopsis:** `audiomirror-cli recv --output <device-id> --bind <addr:port> [flags]`

**Description:**
Listen for UDP Opus packets on the given bind address and play decoded audio to
the specified output device. Runs until interrupted (Ctrl-C). Includes a
configurable jitter buffer for packet reordering and FEC-assisted concealment.

**Flags:**

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--output` | string | *(required)* | Device ID from `audiomirror-cli devices` |
| `--bind` | string | *(required)* | Local bind address, e.g. `0.0.0.0:5004` |
| `--jitter-mode` | `auto\|min` | `auto` | Jitter buffer depth strategy |
| `--jitter-max-depth-ms` | u32 | `100` | Maximum jitter buffer depth in milliseconds |

**Example:**

```sh
audiomirror-cli recv --output "Output:0:Built-in Speakers" --bind 0.0.0.0:5004
```

**Exit codes:**

| Code | Meaning |
|------|---------|
| 0    | Clean shutdown (Ctrl-C) |
| 1    | Device not found, socket bind error, or Opus decoder failure |

**See also:** [send](#send), [devices](#devices)

---

## loop

**Synopsis:** `audiomirror-cli loop --input <device-id> --output <device-id> [flags]`

**Description:**
Single-process loopback test: capture from an input device, encode with Opus,
immediately decode, and play back on an output device. No network involvement.
Useful for verifying audio pipeline integrity and measuring codec latency.

**Flags:**

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--input` | string | *(required)* | Device ID for capture |
| `--output` | string | *(required)* | Device ID for playback |
| `--bitrate` | i32 | `64000` | Opus target bitrate in bits/s |
| `--source` | `mic\|system` | `mic` | Audio source type |
| `--fec-mode` | `auto\|always\|never` | `auto` | Forward-error-correction mode |
| `--simulated-loss-pct` | u8 | `0` | Inject artificial packet loss (0–100) for testing |

**Example:**

```sh
audiomirror-cli loop \
    --input "Input:0:Built-in Microphone" \
    --output "Output:0:Built-in Speakers"
```

**Exit codes:**

| Code | Meaning |
|------|---------|
| 0    | Clean shutdown (Ctrl-C) |
| 1    | Device not found or codec error |

**See also:** [send](#send), [recv](#recv), [devices](#devices)

---

## discover

**Synopsis:** `audiomirror-cli discover [--duration-secs <n>] [--signaling-port <port>]`

**Description:**
Browse for AudioMirror peers on the local network via mDNS
(`_audiomirror._tcp.local.`). Prints discovered peers with their peer ID, name,
address, and version, then exits after the scan window. Does not require a
running daemon.

**Flags:**

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--duration-secs` | u64 | `5` | How long to browse before exiting |
| `--signaling-port` | u16 | `7000` | Local port to advertise in the mDNS record |

**Example:**

```sh
audiomirror-cli discover
audiomirror-cli discover --duration-secs 10
```

**Exit codes:**

| Code | Meaning |
|------|---------|
| 0    | Scan completed (zero or more peers found) |
| 1    | mDNS daemon initialisation error |

**See also:** [daemon](#daemon)

---

## daemon

**Synopsis:** `audiomirror-cli daemon [--signaling-port <port>] [--peer-name <name>]`

**Description:**
Start the AudioMirror background daemon. The daemon binds a TCP signaling server,
registers an mDNS service record, starts a device hot-plug watcher, and
optionally enables a Prometheus metrics endpoint. After startup it prints the
machine-readable banner:

```
READY port=<N>
```

where `<N>` is the actual TCP port (which may differ from `--signaling-port` if
the OS reassigned it). After that line the daemon reads REPL commands from stdin.
See [Daemon REPL protocol](#daemon-repl-protocol).

The daemon loads and respects the persisted settings from the platform config
directory (see `settings show`). Logging is initialised from the persisted
`log_level` setting.

**Flags:**

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--signaling-port` | u16 | `7000` | TCP port for the signaling server |
| `--peer-name` | string | *(from identity file)* | Override the peer name advertised via mDNS |

**Example:**

```sh
audiomirror-cli daemon
audiomirror-cli daemon --signaling-port 5100 --peer-name "Studio Mac"
```

**Exit codes:**

| Code | Meaning |
|------|---------|
| 0    | Clean shutdown (`quit` REPL command or EOF on stdin) |
| 1    | Port bind error, settings load error, or identity file error |

**See also:** [stream open](#stream-open), [discover](#discover), [settings](#settings)

---

## Daemon REPL protocol

Once the daemon has printed `READY port=<N>` it reads line-delimited commands
from stdin. Each command produces structured log output (not plain stdout).
Automation should parse the `READY` line to obtain the port, then write commands
over a pipe.

### Commands

| Command | Arguments | Description |
|---------|-----------|-------------|
| `help` | — | Print the list of available commands to the trace log |
| `peers` | — | List mDNS-discovered peers (index, name, peer-id, host:port, version) |
| `pending` | — | List peers whose `Hello` has been received but not yet accepted or rejected |
| `accept <n>` | n: usize | Accept pending peer at index `<n>` from the `pending` list; adds to TrustStore |
| `connect <peer_id\|name>` | peer_id or peer_name | Initiate an outbound signaling connection to a discovered peer |
| `sessions` | — | List all sessions with state, remote peer ID, and per-stream status |
| `open <peer_id\|name>` | peer_id or peer_name | Open a new session with a connected peer and send `SessionRequest` |
| `disconnect <session_id>` | UUID | Close the session with the given UUID |
| `stream open --from <dev> --to <peer>:<dev> --session <uuid> [--bitrate <bps>]` | see flags | Open an audio stream within an existing session |
| `stream close <session_id>:<stream_id>` | UUID:u8 | Close a specific stream within a session |
| `stream volume <session_id>:<stream_id> <0-100>` | UUID:u8, percent | Set playback volume on a stream (0 = silent, 100 = full) |
| `stream mute <session_id>:<stream_id>` | UUID:u8 | Mute a stream |
| `stream unmute <session_id>:<stream_id>` | UUID:u8 | Unmute a stream |
| `stream pause <session_id>:<stream_id>` | UUID:u8 | Pause a stream |
| `stream resume <session_id>:<stream_id>` | UUID:u8 | Resume a paused stream |
| `stream stats [<session_id>:<stream_id>]` | optional UUID:u8 | Print per-stream statistics once per second; Ctrl-C to stop |
| `quit` | — | Shut down the daemon cleanly |

### Startup line

```
READY port=<N>
```

This is the first line written to stdout after the daemon is fully initialised
(signaling server bound, mDNS registered). `<N>` is a decimal integer. Supervisor
scripts must wait for this line before issuing REPL commands.

---

## stream open

**Synopsis:** `audiomirror-cli stream open --from <device-id> --to <peer-id>:<device-id> [flags]`

**Description:**
Open a peer-to-peer audio stream within an existing session. Requires a running
daemon (`audiomirror-cli daemon`). The source peer captures from `--from` and
sends Opus-encoded audio over UDP to the sink peer's `--to` device. The `--session`
flag identifies which active session to use (obtain a session UUID from the daemon
REPL `sessions` command).

**Flags:**

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--from` | string | *(required)* | Source device ID on the local peer |
| `--to` | string | *(required)* | `<peer-id>:<device-id>` identifying the remote sink |
| `--session` | UUID string | *(required)* | Session UUID (from `sessions` REPL command) |
| `--bitrate` | i32 | `64000` | Opus target bitrate in bits/s |

**Example:**

```sh
# Inside the daemon REPL
stream open --from "Input:0:Built-in Microphone" \
    --to "a1b2c3d4-...:default" \
    --session "e5f6a7b8-..."
```

**Exit codes:**

| Code | Meaning |
|------|---------|
| 0    | Stream active |
| 1    | No running daemon, session not found, or signaling timeout |

**See also:** [daemon](#daemon), [Daemon REPL protocol](#daemon-repl-protocol)

---

## stats

**Synopsis:** `audiomirror-cli stats [--stream-id <id>]`

**Description:**
Display real-time statistics for all active streams managed by the running daemon.
Refreshes once per second. Press Ctrl-C to exit. If `--stream-id` is given, only
that stream is shown. Requires a running daemon.

**Flags:**

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--stream-id` | u8 | *(all)* | Filter output to a single stream ID |

**Example:**

```sh
audiomirror-cli stats
audiomirror-cli stats --stream-id 1
```

**Exit codes:**

| Code | Meaning |
|------|---------|
| 0    | User pressed Ctrl-C |
| 1    | No running daemon |

**See also:** [daemon](#daemon), [stream open](#stream-open)

---

## logs

**Synopsis:** `audiomirror-cli logs <subcommand>`

**Description:**
Inspect the structured application log file written by the daemon and other
subcommands.

### Subcommands

| Subcommand | Description |
|------------|-------------|
| `path` | Print the absolute path of the current log file |
| `tail` | Poll and print new log lines every 200 ms (Ctrl-C to stop) |

**Example:**

```sh
audiomirror-cli logs path
audiomirror-cli logs tail
```

**Exit codes:**

| Code | Meaning |
|------|---------|
| 0    | Success / clean shutdown |
| 1    | Log directory not accessible |

---

## settings

**Synopsis:** `audiomirror-cli settings <subcommand>`

**Description:**
Read or write the persistent application settings stored as TOML in the platform
config directory. Changes take effect on the next daemon start (or immediately
for non-daemon subcommands).

### Subcommands

| Subcommand | Arguments | Description |
|------------|-----------|-------------|
| `show` | — | Print all settings as TOML |
| `get <key>` | key: string | Print the value of a single settings key |
| `set <key> <value>` | key, value: strings | Set a settings key to a new value |

### Known settings keys

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `auto_accept_trusted` | bool | `false` | Automatically accept connections from previously trusted peers |
| `auto_start_with_system` | bool | `false` | Register the daemon as a login/startup item |
| `default_bitrate` | i32 | `64000` | Default Opus bitrate in bits/s |
| `fec_mode` | `auto\|always\|never` | `auto` | Forward-error-correction mode |
| `fec_on_threshold_pct` | u8 | — | Loss percentage above which FEC is enabled (auto mode) |
| `fec_off_threshold_pct` | u8 | — | Loss percentage below which FEC is disabled (auto mode) |
| `fec_hysteresis_secs` | u32 | — | Seconds before switching FEC state (hysteresis) |
| `jitter_mode` | `auto\|min` | `auto` | Jitter buffer depth strategy |
| `jitter_max_depth_ms` | u32 | `100` | Maximum jitter buffer depth in milliseconds |
| `log_level` | `trace\|debug\|info\|warn\|error` | `info` | Minimum log level written to the log file |
| `metrics_enabled` | bool | `false` | Enable the Prometheus `/metrics` HTTP endpoint |
| `metrics_port` | u16 | `9000` | Port for the Prometheus endpoint |

**Example:**

```sh
audiomirror-cli settings show
audiomirror-cli settings get log_level
audiomirror-cli settings set log_level debug
```

**Exit codes:**

| Code | Meaning |
|------|---------|
| 0    | Success |
| 1    | Unknown key, type parse error, or settings file write failure |

---

## autostart

**Synopsis:** `audiomirror-cli autostart <subcommand>`

**Description:**
Manage the platform-native autostart mechanism for the daemon.

| Platform | Mechanism |
|----------|-----------|
| macOS | LaunchAgent plist in `~/Library/LaunchAgents/` |
| Linux | systemd user service unit in `~/.config/systemd/user/` |
| Windows | `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` registry key |

### Subcommands

| Subcommand | Description |
|------------|-------------|
| `enable` | Install the autostart artifact for the current user |
| `disable` | Remove the autostart artifact |
| `status` | Print whether the autostart artifact is present |

**Example:**

```sh
audiomirror-cli autostart enable
audiomirror-cli autostart status
audiomirror-cli autostart disable
```

**Exit codes:**

| Code | Meaning |
|------|---------|
| 0    | Success |
| 1    | Permission error or unsupported platform |

**See also:** [settings](#settings)

---

## metrics

**Synopsis:** `audiomirror-cli metrics <subcommand>`

**Description:**
Manage the optional Prometheus `/metrics` HTTP endpoint. The endpoint is served
by the running daemon; changes to the `metrics_enabled` flag require a daemon
restart to take effect.

### Subcommands

| Subcommand | Description |
|------------|-------------|
| `enable` | Set `metrics_enabled = true` in settings and print a reminder to restart the daemon |
| `disable` | Set `metrics_enabled = false` in settings and print a reminder to restart the daemon |
| `status` | Print `metrics_enabled`, the configured port, and whether the endpoint is currently reachable |

**Example:**

```sh
audiomirror-cli metrics enable
audiomirror-cli metrics status
# Expected output:
# metrics_enabled: true  port: 9000  endpoint_live: true
```

**Exit codes:**

| Code | Meaning |
|------|---------|
| 0    | Success |
| 1    | Settings file error |

**See also:** [settings](#settings)
