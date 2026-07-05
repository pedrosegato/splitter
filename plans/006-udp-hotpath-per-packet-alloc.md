# Plan 006: Eliminate per-packet heap allocation and double copy on the UDP data path

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat 217a31d..HEAD -- crates/splitter-core/src/net/stream_runtime.rs crates/splitter-core/src/net/packet.rs`
> If either in-scope file changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Priority**: P2
- **Effort**: S
- **Risk**: LOW
- **Depends on**: none
- **Category**: perf
- **Planned at**: commit `217a31d`, 2026-07-05

## Why this matters

The UDP send and receive pumps run once per 20 ms audio packet per stream. Today
each outbound packet does **one heap allocation plus two copies** of the Opus
payload, and each inbound packet does **one heap allocation** — all on the hot
path, for the lifetime of every session. On the send side the payload is copied
into a throwaway `Bytes`, then copied *again* into the wire buffer; the `Bytes`
is discarded immediately. On the recv side a fresh `Bytes` is allocated for every
datagram even though a reusable receive buffer already exists. Removing these
gives steadier latency and less allocator pressure under multi-stream load, with
no protocol or behavior change. A lock-free `BufferPool` already exists in
`crates/splitter-core/src/audio/buffers.rs` but has zero call sites; this plan
does **not** wire it up — the fix below removes the allocations outright without a
pool, which is simpler and leaves less surface to get wrong.

## Current state

Files:

- `crates/splitter-core/src/net/packet.rs` — wire format. `Packet { stream_id: u8,
  seq: u32, timestamp_ms: u32, payload: Bytes }`, `HEADER_LEN = 10`,
  `MAX_PACKET_LEN = 1500`. `encode(&self, out: &mut BytesMut)` and
  `decode(buf: Bytes)` (lines 15–67). Header layout: `stream_id`(u8) +
  3-byte big-endian seq + `timestamp_ms`(u32 BE) + `payload_len`(u16 BE) + payload.
- `crates/splitter-core/src/net/stream_runtime.rs` — the two async pumps. Imports
  `use bytes::{Bytes, BytesMut};` at line 8.

Outbound pump — `spawn_source_pump_inner` (the allocation + double copy),
`stream_runtime.rs:599-618`:

```rust
if let Err(e) = encoder.encode(&frame, &mut payload) {
    tracing::warn!("opus encode failed: {e}");
} else {
    let pkt = Packet {
        stream_id: stream_id.get(),
        seq: seq & SEQ_MASK,
        timestamp_ms: start.elapsed().as_millis() as u32,
        payload: Bytes::copy_from_slice(&payload[..]),   // <-- alloc + copy #1
    };
    if pkt.encode(&mut packet_buf).is_ok() {              // <-- copy #2 (put_slice)
        match socket.send(&packet_buf[..]).await {
```

Here `payload` is a reused `BytesMut` (`let mut payload = BytesMut::with_capacity(400);`,
line 573) that the Opus encoder writes into, and `packet_buf` is a reused
`BytesMut` (line 574). The `Bytes::copy_from_slice` is pure waste: the bytes are
immediately re-copied into `packet_buf` by `Packet::encode` (`out.put_slice(&self.payload)`,
packet.rs:35) and the `Bytes` is dropped.

Inbound pump — `spawn_sink_pump_inner` (the per-datagram allocation),
`stream_runtime.rs:661-700`:

```rust
recv_res = socket.recv(&mut buf) => {
    let n = match recv_res { Ok(n) => n, Err(e) => { /* warn; continue */ } };
    let bytes = Bytes::copy_from_slice(&buf[..n]);   // <-- alloc + copy per datagram
    let pkt = match Packet::decode(bytes) {
        Ok(p) if p.stream_id == stream_id.get() => p,
        Ok(_) => continue,
        Err(e) => { tracing::warn!("packet decode failed: {e}"); continue; }
    };
    stats.packets_received.fetch_add(1, Ordering::Relaxed);
    ...
    stats.last_seq_received.store(pkt.seq, Ordering::Relaxed);
    // gap detection uses pkt.seq ...
    if decoder.decode(Some(&pkt.payload[..]), &mut decoded).is_ok() { ... }
}
```

`buf` is a reused `let mut buf = vec![0u8; 1500];` (line 646). `Packet::decode`
does `buf.slice(..payload_len)` which is zero-copy on the `Bytes`, so the *only*
real cost is the `Bytes::copy_from_slice`. The pump only ever reads
`pkt.stream_id`, `pkt.seq`, and `pkt.payload` as a `&[u8]` — it never needs an
owned `Bytes`.

`packet.rs` already validates on encode: `total > MAX_PACKET_LEN` →
`NetError::PayloadLenMismatch`, `seq > 0xFF_FFFF` → `NetError::SeqOverflow`
(packet.rs:16-37). These checks MUST be preserved.

### Conventions that apply here

- **No code comments** except a non-obvious *why* (project rule in `CLAUDE.md`).
  Do not narrate the byte-packing.
- Error type is `crate::error::NetError`; reuse the existing variants
  `PayloadLenMismatch { declared, available }` and `SeqOverflow { seq }`.
- Big-endian on the wire (`put_u8`/`put_u32`/`put_u16` in the current encoder).
- Tests live in a `#[cfg(test)] mod tests` at the bottom of `packet.rs`; model new
  tests on `encode_decode_roundtrip_simple` (packet.rs:74-88) and the existing
  `proptest_roundtrip` (packet.rs:134-147).

## Commands you will need

| Purpose   | Command                                                              | Expected on success   |
|-----------|---------------------------------------------------------------------|-----------------------|
| Build     | `cargo build --workspace`                                           | exit 0                |
| Tests     | `cargo test -p splitter-core net::packet`                          | all pass              |
| Tests     | `cargo test -p splitter-core stream_runtime`                       | all pass              |
| Tests     | `cargo test --workspace`                                            | all pass              |
| Lint      | `cargo clippy --workspace --all-targets -- -D warnings`            | exit 0, no warnings   |
| Format    | `cargo fmt --all -- --check`                                        | exit 0                |

## Scope

**In scope** (the only files you should modify):
- `crates/splitter-core/src/net/packet.rs`
- `crates/splitter-core/src/net/stream_runtime.rs`

**Out of scope** (do NOT touch, even though they look related):
- `crates/splitter-core/src/audio/buffers.rs` — `BufferPool` stays untouched. It
  is currently dead; wiring it up is a separate decision and is not needed to fix
  this finding. Do not delete it either (out of scope).
- The `Packet` struct's public field layout and the wire format — bytes on the
  wire must be byte-for-byte identical before and after. The existing
  `proptest_roundtrip` is your guard.
- The gap-detection / packet-loss-concealment logic in the sink pump
  (stream_runtime.rs:682-700) — leave its behavior unchanged; only change how the
  packet is decoded.

## Git workflow

- Branch: `advisor/006-udp-hotpath-per-packet-alloc`
- Commit style: conventional-commit **title only**, no body. Example from
  `git log`: `refactor(types): newtype SessionId`. A fitting title here:
  `perf(net): drop per-packet alloc on udp send/recv path`.
- **NEVER** add a `Co-Authored-By` trailer of any kind.
- Do NOT push or open a PR.

## Steps

### Step 1: Add `Packet::encode_from_parts` and route `encode` through it

In `packet.rs`, add an associated function that writes header + payload straight
into `out` from borrowed parts, with the *same* validation as `encode`. Then make
the existing `encode` delegate to it so there is a single source of truth.

Target shape:

```rust
impl Packet {
    pub fn encode_from_parts(
        stream_id: u8,
        seq: u32,
        timestamp_ms: u32,
        payload: &[u8],
        out: &mut BytesMut,
    ) -> Result<usize, NetError> {
        let total = HEADER_LEN + payload.len();
        if total > MAX_PACKET_LEN {
            return Err(NetError::PayloadLenMismatch {
                declared: payload.len(),
                available: MAX_PACKET_LEN - HEADER_LEN,
            });
        }
        if seq > 0xFF_FFFF {
            return Err(NetError::SeqOverflow { seq });
        }
        out.clear();
        out.reserve(total);
        out.put_u8(stream_id);
        out.put_u8(((seq >> 16) & 0xFF) as u8);
        out.put_u8(((seq >> 8) & 0xFF) as u8);
        out.put_u8((seq & 0xFF) as u8);
        out.put_u32(timestamp_ms);
        out.put_u16(payload.len() as u16);
        out.put_slice(payload);
        Ok(total)
    }

    pub fn encode(&self, out: &mut BytesMut) -> Result<usize, NetError> {
        Self::encode_from_parts(self.stream_id, self.seq, self.timestamp_ms, &self.payload, out)
    }
}
```

**Verify**: `cargo test -p splitter-core net::packet` → all existing packet tests
still pass (the `encode`-based tests now exercise the delegated path).

### Step 2: Add a borrowing decode `Packet::decode_ref` returning a `PacketView`

In `packet.rs`, add a zero-allocation view type and a decoder that borrows the
input slice. Do not change `decode(buf: Bytes)` — leave it as-is for callers that
own a `Bytes`.

Target shape:

```rust
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PacketView<'a> {
    pub stream_id: u8,
    pub seq: u32,
    pub timestamp_ms: u32,
    pub payload: &'a [u8],
}

impl Packet {
    pub fn decode_ref(buf: &[u8]) -> Result<PacketView<'_>, NetError> {
        if buf.len() < HEADER_LEN {
            return Err(NetError::HeaderTruncated {
                got: buf.len(),
                need: HEADER_LEN,
            });
        }
        let stream_id = buf[0];
        let seq = ((buf[1] as u32) << 16) | ((buf[2] as u32) << 8) | buf[3] as u32;
        let timestamp_ms = u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]);
        let payload_len = u16::from_be_bytes([buf[8], buf[9]]) as usize;
        let rest = &buf[HEADER_LEN..];
        if rest.len() < payload_len {
            return Err(NetError::PayloadLenMismatch {
                declared: payload_len,
                available: rest.len(),
            });
        }
        Ok(PacketView {
            stream_id,
            seq,
            timestamp_ms,
            payload: &rest[..payload_len],
        })
    }
}
```

Use the exact `NetError` variants and field names shown (they match the existing
`decode`: `HeaderTruncated { got, need }`, `PayloadLenMismatch { declared, available }`).

**Verify**: `cargo build --workspace` → exit 0.

### Step 3: Switch the outbound pump to `encode_from_parts` (no intermediate `Bytes`)

In `stream_runtime.rs`, in `spawn_source_pump_inner`, replace the `Packet { .. }`
construction + `pkt.encode(&mut packet_buf)` (lines ~602-608) with a single call
that writes directly into `packet_buf`:

```rust
} else {
    let encoded = Packet::encode_from_parts(
        stream_id.get(),
        seq & SEQ_MASK,
        start.elapsed().as_millis() as u32,
        &payload[..],
        &mut packet_buf,
    );
    if encoded.is_ok() {
        match socket.send(&packet_buf[..]).await {
            Ok(n) => {
                stats.packets_sent.fetch_add(1, Ordering::Relaxed);
                stats.bytes_sent.fetch_add(n as u64, Ordering::Relaxed);
                seq = seq.wrapping_add(1);
            }
            Err(e) => tracing::warn!("udp send failed: {e}"),
        }
    }
}
```

Keep the surrounding `if consumer.occupied() >= FRAME_STEREO_SAMPLES { frame_ready.notify_one(); }`
tail unchanged.

**Verify**: `cargo build --workspace` → exit 0.

### Step 4: Switch the inbound pump to `decode_ref` over the reused `buf`

In `spawn_sink_pump_inner`, replace lines ~669-677:

```rust
let bytes = Bytes::copy_from_slice(&buf[..n]);
let pkt = match Packet::decode(bytes) {
    Ok(p) if p.stream_id == stream_id.get() => p,
    Ok(_) => continue,
    Err(e) => { tracing::warn!("packet decode failed: {e}"); continue; }
};
```

with a borrowing decode:

```rust
let pkt = match Packet::decode_ref(&buf[..n]) {
    Ok(p) if p.stream_id == stream_id.get() => p,
    Ok(_) => continue,
    Err(e) => {
        tracing::warn!("packet decode failed: {e}");
        continue;
    }
};
```

Everything downstream (`pkt.seq`, `pkt.payload`, the gap/loss logic, and
`decoder.decode(Some(&pkt.payload[..]), &mut decoded)`) reads the same field names
via `PacketView` and needs no change. `pkt` now borrows `buf` for the rest of the
match arm; that is fine because `buf` is only written again on the next loop
iteration, after `pkt` is dropped.

**Verify**: `cargo test -p splitter-core stream_runtime` → all pass (including the
existing `sink_pump_tests`).

### Step 5: Fix the now-unused `Bytes` import

After Steps 3–4, the top-level `use bytes::{Bytes, BytesMut};` (line 8) no longer
uses `Bytes` (the only top-level uses were the two `Bytes::copy_from_slice` calls
you removed; the test module at line 732 has its own import). Change line 8 to:

```rust
use bytes::BytesMut;
```

**Verify**: `cargo clippy --workspace --all-targets -- -D warnings` → exit 0, no
`unused_imports` warning.

## Test plan

- In `packet.rs` `#[cfg(test)] mod tests`, add:
  - `encode_from_parts_matches_struct_encode`: build a `Packet`, encode it into
    `buf_a` via `pkt.encode`; encode the same fields via
    `Packet::encode_from_parts(..)` into `buf_b`; assert `buf_a == buf_b` and both
    round-trip via `Packet::decode`.
  - `decode_ref_matches_decode`: encode a known packet, then assert
    `Packet::decode_ref(&buf[..])` yields a `PacketView` whose fields equal the
    corresponding `Packet::decode(buf.freeze())` fields (compare `stream_id`,
    `seq`, `timestamp_ms`, and `payload` bytes).
  - `decode_ref_too_short_errors`: `Packet::decode_ref(b"\x01\x02")` →
    `Err(NetError::HeaderTruncated { .. })`.
  - `decode_ref_payload_len_mismatch_errors`: build a header claiming a longer
    payload than present → `Err(NetError::PayloadLenMismatch { .. })`.
  - `encode_from_parts_seq_overflow_errors`: `seq = 0x0100_0000` →
    `Err(NetError::SeqOverflow { .. })`.
  - `encode_from_parts_too_large_errors`: `payload` of `MAX_PACKET_LEN` bytes →
    `Err(NetError::PayloadLenMismatch { .. })`.
  - Extend `proptest_roundtrip` (or add a sibling proptest) to also assert that
    `encode_from_parts` produces bytes identical to `encode`, and that
    `decode_ref` agrees with `decode` on the same buffer.
- Model structure on `encode_decode_roundtrip_simple` (packet.rs:74-88) and
  `proptest_roundtrip` (packet.rs:134-147).
- Existing `sink_pump_tests` in `stream_runtime.rs` (starts line 725) must stay
  green unchanged — they are the integration guard for the recv-path swap.
- Verification: `cargo test --workspace` → all pass, including the new packet tests.

## Done criteria

Machine-checkable. ALL must hold:

- [ ] `cargo build --workspace` exits 0
- [ ] `cargo test --workspace` exits 0; the new `packet.rs` tests exist and pass
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` exits 0
- [ ] `cargo fmt --all -- --check` exits 0
- [ ] `grep -n "Bytes::copy_from_slice" crates/splitter-core/src/net/stream_runtime.rs`
      returns **only** matches inside the `#[cfg(test)]` modules (the two hot-path
      uses at the old lines 606 and 669 are gone)
- [ ] No files outside the in-scope list are modified (`git status`)
- [ ] `plans/README.md` status row for 006 updated

## STOP conditions

Stop and report back (do not improvise) if:

- The drift check shows either in-scope file changed and the "Current state"
  excerpts no longer match the live code.
- The wire-format proptest (`proptest_roundtrip`, or your new equivalence
  proptest) fails — this means the byte layout diverged; do not "fix" the test.
- Removing the intermediate `Bytes` on the send path forces a borrow-checker
  change to any code outside `spawn_source_pump_inner` (it should not).
- You find that `Packet::decode` (the owned-`Bytes` variant) has other callers
  that you would need to change — this plan intentionally leaves `decode` intact;
  if a caller must move to `decode_ref`, that is out of scope — report it.

## Maintenance notes

- `Packet::encode` and `Packet::encode_from_parts` now share one implementation;
  any future header-layout change must be made once in `encode_from_parts` and the
  `proptest` equivalence test will catch a divergence.
- `PacketView` borrows its input buffer. If a future caller needs to keep the
  packet past the lifetime of the receive buffer, it must copy the payload
  explicitly (or use the owned `Packet::decode`); do not add a lifetime-laundering
  `unsafe` to work around it.
- `BufferPool` in `audio/buffers.rs` remains unused after this change. A reviewer
  deciding its fate should note this plan removed the allocations it would have
  pooled, so its remaining justification is elsewhere (or it is deletable).
- Reviewer scrutiny: confirm the send path still validates `MAX_PACKET_LEN` and
  `SeqOverflow` (via `encode_from_parts`) — a payload/seq that previously errored
  must still error, not silently truncate.
