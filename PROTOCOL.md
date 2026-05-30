# Splitter Protocol Reference

Wire-format and signaling protocol reference for engineers. This document
distils the implementation in `crates/splitter-core/src/net/` and does not
repeat the product narrative in `SPEC.md`.

---

## Table of Contents

1. [Audio UDP packet](#1-audio-udp-packet)
2. [Signaling transport](#2-signaling-transport)
3. [Signaling JSON messages](#3-signaling-json-messages)
4. [Session lifecycle](#4-session-lifecycle)
5. [Heartbeat timing](#5-heartbeat-timing)
6. [mDNS service discovery](#6-mdns-service-discovery)

---

## 1. Audio UDP packet

Source: `crates/splitter-core/src/net/packet.rs`

```
pub const HEADER_LEN: usize = 10;
pub const MAX_PACKET_LEN: usize = 1500;
```

### 1.1 Byte layout

```
 0                   1                   2                   3
 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
| stream_id (8) |        seq[23:16]     |     seq[15:8]         |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|   seq[7:0]    |              timestamp_ms (32 bits, big-endian)
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
               timestamp_ms (continued)                         |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|         payload_len (16 bits, big-endian)    | payload ...    |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
```

### 1.2 Field table

| Offset (bytes) | Size (bytes) | Field | Encoding | Description |
|---------------|--------------|-------|----------|-------------|
| 0 | 1 | `stream_id` | u8 big-endian | Identifies the audio stream; matches `--stream-id` flag |
| 1 | 3 | `seq` | u24 big-endian | Packet sequence number; wraps at 2²⁴ = 16 777 216 |
| 4 | 4 | `timestamp_ms` | u32 big-endian | Milliseconds since the sender's epoch (not wall clock) |
| 8 | 2 | `payload_len` | u16 big-endian | Length in bytes of the Opus payload that follows |
| 10 | `payload_len` | payload | raw bytes | Opus-encoded audio frame (20 ms at 48 kHz) |

Total header size: **10 bytes** (`HEADER_LEN`).

### 1.3 MTU and size limits

- Maximum packet length is **1500 bytes** (`MAX_PACKET_LEN`), matching Ethernet
  MTU minus IP/UDP overhead. The encoder must not produce payloads larger than
  `1500 − 10 = 1490 bytes`.
- `encode()` returns `NetError::PayloadLenMismatch` if the total would exceed
  `MAX_PACKET_LEN`.

### 1.4 Sequence number wrap-around

`seq` is a **u24** (24-bit unsigned integer). The valid range is `0..=0xFF_FFFF`
(0 to 16 777 215 inclusive). On the sender side:

```rust
let pkt = Packet { seq: seq & 0xFF_FFFF, .. };
seq = seq.wrapping_add(1);
```

The receiver must treat a jump larger than half the range (8 388 608) as a
wrap-around, not a large gap, when comparing sequence numbers for jitter buffer
ordering.

---

## 2. Signaling transport

The signaling channel uses **TCP** with
[`LengthDelimitedCodec`](https://docs.rs/tokio-util/latest/tokio_util/codec/struct.LengthDelimitedCodec.html)
framing (`max_frame_length = 1 MiB`). Each frame carries exactly one
length-prefixed JSON object that deserialises to a `SignalingMessage` variant.

The server listens on the port specified via `--signaling-port` (default **7000**).
The actual bound port is reported in the `READY port=<N>` startup line.

---

## 3. Signaling JSON messages

Source: `crates/splitter-core/src/net/signaling/message.rs`

All messages are tagged with `"type"` using `serde(tag = "type", rename_all = "snake_case")`.
Optional fields are omitted from serialised output when absent (`skip_serializing_if = "Option::is_none"`).

### Protocol version

```rust
pub const PROTOCOL_VERSION: u32 = 1;
```

### Supporting types

**`Capabilities`**

| Field | Type | Description |
|-------|------|-------------|
| `codecs` | `Vec<String>` | Supported codec names, e.g. `["opus"]` |
| `max_streams` | u32 | Maximum concurrent streams the peer supports |

**`Endpoint`**

| Field | Type | Description |
|-------|------|-------------|
| `peer_id` | String (UUID) | Peer UUID |
| `device_id` | String | Device ID on that peer; `"default"` selects the default output |

**`CodecParams`**

| Field | Type | Description |
|-------|------|-------------|
| `name` | String | Codec name, always `"opus"` in current implementation |
| `bitrate` | i32 | Target bitrate in bits/s |
| `frame_ms` | u32 | Frame duration in milliseconds (always `20`) |

**`StreamAction`** (`serde(rename_all = "snake_case")`)

| Variant | JSON value |
|---------|-----------|
| `Pause` | `"pause"` |
| `Resume` | `"resume"` |
| `Close` | `"close"` |
| `SetVolume` | `"set_volume"` |

**`HeartbeatStreamStats`**

| Field | Type | Optional | Description |
|-------|------|----------|-------------|
| `stream_id` | u8 | No | Stream identifier |
| `packets_sent` | u64 | No | Cumulative packets sent |
| `packets_received` | u64 | No | Cumulative packets received |
| `packets_lost` | u64 | No | Cumulative packets declared lost |
| `rtt_ms` | u32 | Yes | Last measured round-trip time in milliseconds; absent when unavailable |

---

### 3.1 `hello`

Sent by the connecting peer immediately after the TCP connection is established.

```json
{
  "type": "hello",
  "protocol_version": 1,
  "peer_id": "<uuid>",
  "peer_name": "Studio Mac",
  "app_version": "0.1.0",
  "capabilities": { "codecs": ["opus"], "max_streams": 4 },
  "auth_token": "<base64-random-32-bytes>"
}
```

| Field | Type | Description |
|-------|------|-------------|
| `protocol_version` | u32 | Must equal `PROTOCOL_VERSION` (1) |
| `peer_id` | String (UUID) | Sender's persistent peer ID |
| `peer_name` | String | Human-readable peer name |
| `app_version` | String | Cargo package version string |
| `capabilities` | Capabilities | Codec and stream limits |
| `auth_token` | String | Base64-encoded 32-byte random token; stored in TrustStore after first acceptance (TOFU) |

---

### 3.2 `hello_ack`

Sent by the server in response to `hello`. If `accepted` is false, the
connection is closed after this message.

```json
{ "type": "hello_ack", "accepted": true }
{ "type": "hello_ack", "accepted": false, "reason": "not trusted" }
```

| Field | Type | Optional | Description |
|-------|------|----------|-------------|
| `accepted` | bool | No | `true` if the server accepts the peer |
| `reason` | String | Yes | Human-readable rejection reason |

---

### 3.3 `session_request`

Sent by the initiator to request a new session. The responder replies with
`session_response`.

```json
{
  "type": "session_request",
  "session_id": "<uuid>",
  "requested_by": "<peer-uuid>"
}
```

| Field | Type | Description |
|-------|------|-------------|
| `session_id` | String (UUID) | New session identifier chosen by the initiator |
| `requested_by` | String (UUID) | Peer ID of the requesting side |

---

### 3.4 `session_response`

Sent by the responder to accept or reject a `session_request`. Also used to
signal session closure.

```json
{ "type": "session_response", "session_id": "<uuid>", "accepted": true }
{ "type": "session_response", "session_id": "<uuid>", "accepted": false }
```

| Field | Type | Description |
|-------|------|-------------|
| `session_id` | String (UUID) | Identifies the session |
| `accepted` | bool | `true` = accepted / active; `false` = rejected or closed |

---

### 3.5 `stream_open`

Sent by the stream initiator to negotiate a new audio stream within a session.
The `udp_port` field carries the port on which the initiator will send audio;
the receiver replies with `stream_open_ack` containing its own UDP listen port.

```json
{
  "type": "stream_open",
  "session_id": "<uuid>",
  "stream_id": 0,
  "source": { "peer_id": "<uuid>", "device_id": "Input:0:Built-in Microphone" },
  "sink":   { "peer_id": "<uuid>", "device_id": "default" },
  "codec":  { "name": "opus", "bitrate": 64000, "frame_ms": 20 },
  "udp_port": 0
}
```

| Field | Type | Description |
|-------|------|-------------|
| `session_id` | String (UUID) | Owning session |
| `stream_id` | u8 | Stream index within the session |
| `source` | Endpoint | Capturing peer and device ID |
| `sink` | Endpoint | Playback peer and device ID |
| `codec` | CodecParams | Codec negotiation |
| `udp_port` | u16 | Initiator's UDP port (0 = ephemeral; the sink uses the ack port for its own bind) |

---

### 3.6 `stream_open_ack`

Sent by the stream acceptor in response to `stream_open`.

```json
{ "type": "stream_open_ack", "stream_id": 0, "accepted": true,  "udp_port": 48210 }
{ "type": "stream_open_ack", "stream_id": 0, "accepted": false }
```

| Field | Type | Optional | Description |
|-------|------|----------|-------------|
| `stream_id` | u8 | No | Echoes the `stream_id` from `stream_open` |
| `accepted` | bool | No | `true` if the sink is ready |
| `udp_port` | u16 | Yes | UDP port on which the sink is listening; absent when `accepted` is false |

---

### 3.7 `stream_control`

Sent by either peer to pause, resume, close, or adjust the volume of an active
stream.

```json
{ "type": "stream_control", "stream_id": 0, "action": "pause" }
{ "type": "stream_control", "stream_id": 0, "action": "set_volume", "volume": 0.75 }
{ "type": "stream_control", "stream_id": 0, "action": "close" }
```

| Field | Type | Optional | Description |
|-------|------|----------|-------------|
| `stream_id` | u8 | No | Target stream |
| `action` | StreamAction | No | One of `pause`, `resume`, `close`, `set_volume` |
| `volume` | f32 | Yes | Linear gain 0.0–1.0; only present when `action = "set_volume"` |

---

### 3.8 `heartbeat`

Sent by each peer every **1 second** while the connection is open. Carries
per-stream statistics and a timestamp for RTT measurement.

```json
{
  "type": "heartbeat",
  "timestamp_ms": 1234567,
  "streams_stats": [
    {
      "stream_id": 0,
      "packets_sent": 500,
      "packets_received": 495,
      "packets_lost": 5,
      "rtt_ms": 12
    }
  ]
}
```

| Field | Type | Description |
|-------|------|-------------|
| `timestamp_ms` | u64 | Sender's monotonic millisecond timestamp |
| `streams_stats` | `Vec<HeartbeatStreamStats>` | One entry per active stream; empty array when no streams are active |

---

## 4. Session lifecycle

```
Initiator                                        Responder
    |                                                 |
    |------- TCP connect --------------------------->|
    |------- Hello --------------------------------->|
    |<------ HelloAck (accepted=true) ---------------|
    |                                                 |
    |------- SessionRequest ----------------------->|
    |<------ SessionResponse (accepted=true) --------|
    |                                                 |
    |------- StreamOpen --------------------------->|  (source → sink)
    |<------ StreamOpenAck (accepted=true, port) ----|
    |                                                 |
    |====== [UDP audio packets flowing] ==============|
    |                                                 |
    |------- Heartbeat (every 1 s) -------------->  |
    |<------ Heartbeat (every 1 s) -----------------|
    |                                                 |
    |------- StreamControl (action=close) -------->|
    |                                                 |
    |------- SessionResponse (accepted=false) ----->|  (close signal)
    |                                                 |
    |====== [TCP connection closed] ===================|
```

Notes:
- A rejected `HelloAck` (`accepted=false`) causes the TCP connection to close
  immediately.
- A rejected `SessionResponse` (`accepted=false`) closes the session; existing
  streams are stopped.
- Multiple `StreamOpen` / `StreamOpenAck` exchanges may occur within one session
  (one per audio stream).
- Either peer may send `StreamControl(action=close)` to stop a stream without
  ending the session.

---

## 5. Heartbeat timing

Source: `crates/splitter-core/src/net/signaling/connection.rs`

| Parameter | Value | Notes |
|-----------|-------|-------|
| Interval | **1 000 ms** | `tokio::time::interval(Duration::from_secs(1))` |
| Timeout | **5 000 ms** | `REMOTE_PEER_HEARTBEAT_TIMEOUT = Duration::from_secs(5)` |
| Check period | **500 ms** | Deadline check fires every 500 ms; triggers disconnect if `last_heard > 5 s` ago |

If no heartbeat (or any other message) is received within 5 seconds, the
connection task emits a `PeerEvent::Disconnected` and tears down the TCP
connection.

---

## 6. mDNS service discovery

Source: `crates/splitter-core/src/net/discovery.rs`

### Service type

```
_splitter._tcp.local.
```

### TXT record keys

| Key | Type | Description |
|-----|------|-------------|
| `peer_id` | String (UUID) | Persistent peer identifier |
| `peer_name` | String | Human-readable name set via `--peer-name` or the identity file |
| `version` | String | Application version, e.g. `"0.1.0"` |
| `signaling_port` | String (u16) | TCP port of the signaling server |

### Service instance name

The instance name in the DNS-SD record is the peer UUID string:

```
<peer-uuid>._splitter._tcp.local.
```

The hostname is `<peer-uuid>.local.` with address auto-detection enabled
(`enable_addr_auto()`).

### `signaling_port` derivation

The daemon's `--signaling-port` flag (default **7000**) is passed directly into
the mDNS `ServiceInfo` as both the SRV port and the `signaling_port` TXT value.
Discovering clients use the TXT value to form the TCP connection address.

### Discovery flow

1. The discovering client queries for `_splitter._tcp.local.`.
2. Each resolved `ServiceEvent::ServiceResolved` yields a `DiscoveredPeer`
   struct with the fields above, plus the resolved IP address from the SRV/A
   record.
3. Self-records are filtered out by comparing `fullname` with the local
   registration.
4. `ServiceEvent::ServiceRemoved` events emit `DiscoveryEvent::Removed`.
