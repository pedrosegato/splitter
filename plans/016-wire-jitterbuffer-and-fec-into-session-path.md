# Plan 016: Route the P2P session data path through JitterBuffer and in-band FEC

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` if that file exists (it may not yet — if absent, skip).
>
> **Drift check (run first)**:
> `git diff --stat 217a31d..HEAD -- crates/splitter-core/src/net/stream_runtime.rs crates/splitter-core/src/net/jitter.rs crates/splitter-core/src/net/fec.rs crates/splitter-core/src/audio/codec.rs crates/splitter-core/tests/stream_data_plane.rs`
> If any of those files changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Priority**: P2
- **Effort**: L
- **Risk**: MED
- **Depends on**: none — but plan 017 (splitting `stream_runtime.rs`) MUST be
  rebased on top of this. Do 016 first, then 017. See "Maintenance notes".
- **Category**: bug
- **Planned at**: commit `217a31d`, 2026-07-05

## Why this matters

The real peer-to-peer audio session path **bypasses the JitterBuffer and the
FEC controller entirely**. Both modules exist, are unit-tested, and are wired
into the CLI dev commands (`recv.rs`, `send.rs`) — but the actual session pumps
in `stream_runtime.rs` decode each UDP datagram the instant it arrives, with no
reordering stage and no forward-error-correction. Three concrete defects follow
from this:

1. **No reordering.** Any packet that arrives out of order is decoded out of
   order, producing audible glitches on real networks (Wi-Fi, congested LAN).
2. **A cursor-regression bug.** On a sequence gap the sink pump runs a manual
   PLC loop, then unconditionally does `last_seq = Some(pkt.seq)`. When the gap
   was caused by a *late* (out-of-order) packet, this regresses the sequence
   cursor, so the next genuinely-in-order packet is mis-scored as a huge gap
   and triggers a spurious PLC burst.
3. **Negotiated FEC is inert.** The source pump never calls `encoder.set_fec`,
   so the in-band FEC the two peers negotiate is never actually enabled, and
   the sink always calls `decode(Some(payload))` (never the FEC-recovery path).

After this plan lands, received packets flow through `JitterBuffer::push` /
`pop_ready` (reorder + loss detection), the cursor-regression bug is gone,
lost frames are recovered via Opus in-band FEC where possible, and the source
side actually enables FEC. The wiring mirrors the already-proven CLI reference
in `recv.rs` / `send.rs`.

Because ordered packets pass through `pop_ready` with **zero added hold-back**
(see "Current state" — `pop_ready` returns an in-order packet immediately and
never delays to fill `target_depth`), steady-state latency on a clean network
is essentially unchanged. Added latency is incurred only on actual reordering
or loss, bounded by `max_depth_ms` (200 ms).

## Current state

### Files

- `crates/splitter-core/src/net/stream_runtime.rs` — hosts the two real-time
  pump loops. `spawn_source_pump_inner` (lines 553–627) encodes+sends;
  `spawn_sink_pump_inner` (lines 629–704) recv+decodes. `apply_gain_and_push`
  (lines 706–723) is the shared "attenuate then push to playback ring" helper.
  **This is the only production file this plan modifies.**
- `crates/splitter-core/src/net/jitter.rs` — `JitterBuffer` (reorder buffer).
  Do NOT modify; only call its API.
- `crates/splitter-core/src/net/fec.rs` — `FecController` (loss→FEC decision).
  Do NOT modify; only call its API.
- `crates/splitter-core/src/audio/codec.rs` — `OpusEncoder::set_fec`,
  `OpusDecoder::decode` / `decode_with_fec`. Do NOT modify.
- `crates/splitter-cli/src/commands/recv.rs` — **reference wiring** for the
  sink side (jitter push/pop + FEC decode). Read it; do not modify.
- `crates/splitter-cli/src/commands/send.rs` and
  `crates/splitter-cli/src/commands/audio_pipeline.rs` — **reference wiring**
  for the source side (FecController + `set_fec`). Read; do not modify.
- `crates/splitter-core/tests/stream_data_plane.rs` — existing data-plane
  integration test; you will ADD one test here.

### The sink pump as it exists today (the bug), `stream_runtime.rs:629-704`

```rust
pub async fn spawn_sink_pump_inner(
    _session_id: SessionId,
    stream_id: StreamId,
    socket: UdpSocket,
    mut producer: RingProducer,
    mut control_rx: mpsc::Receiver<StreamControlSignal>,
    stats: Arc<StreamStats>,
) {
    let mut decoder = match OpusDecoder::new() { /* ... */ };
    let mut decoded = vec![0.0f32; FRAME_STEREO_SAMPLES];
    let mut buf = vec![0u8; 1500];
    let mut last_seq: Option<u32> = None;      // <-- to be removed
    let mut gain: f32 = 1.0;
    let muted = Arc::new(AtomicBool::new(false));
    let mut paused = false;

    loop {
        tokio::select! {
            biased;
            maybe_sig = control_rx.recv() => { /* apply_control, unchanged */ }
            recv_res = socket.recv(&mut buf) => {
                // decode Packet, bump packets_received/bytes_received/last_seq_received
                // ...
                if let Some(prev) = last_seq {          // <-- manual PLC + cursor-regression bug
                    let expected = prev.wrapping_add(1) & SEQ_MASK;
                    if pkt.seq != expected {
                        let lost = seq_gap(expected, pkt.seq) as u64;
                        if lost > 0 && lost < 100 {
                            for _ in 0..lost {
                                if decoder.decode(None, &mut decoded).is_ok() {
                                    stats.packets_lost.fetch_add(1, Ordering::Relaxed);
                                    apply_gain_and_push(&mut decoded, gain, muted.load(Ordering::Relaxed), paused, &mut producer);
                                }
                            }
                        }
                    }
                }
                last_seq = Some(pkt.seq);                // <-- REGRESSES on a late packet

                if decoder.decode(Some(&pkt.payload[..]), &mut decoded).is_ok() {   // <-- never FEC
                    apply_gain_and_push(&mut decoded, gain, muted.load(Ordering::Relaxed), paused, &mut producer);
                }
            }
        }
    }
}
```

### The reference sink wiring, `recv.rs:31-83` (mirror this)

```rust
loop {
    let n = sock.recv(&mut udp_buf).await?;
    let pkt = Packet::decode(Bytes::copy_from_slice(&udp_buf[..n]))?; // (error handled)
    let now = std::time::Instant::now();
    jitter.push(pkt, now);

    while let Some(out) = jitter.pop_ready(now) {
        match out {
            JitterOutput::Lost { seq } => { pending_fec_recover = true; /* + count loss */ }
            JitterOutput::Packet(p) => {
                handle_packet(&mut decoder, &mut producer, &p.payload,
                              &mut pending_fec_recover, &mut frame);
            }
        }
    }
}
// handle_packet: if pending_fec_recover { decode_with_fec(Some(payload), frame, true) -> push; clear }
//                then decode_with_fec(Some(payload), frame, false) -> push
```

Note this reference uses **push-then-drain** with NO separate timer tick. This
is the pattern to reproduce — see "Design decision" below.

### `JitterBuffer` API (`jitter.rs`), do not change

```rust
pub enum JitterOutput { Packet(Packet), Lost { seq: u32 } }
impl JitterBuffer {
    pub fn new(mode: JitterMode, max_depth_ms: u32) -> Self;
    pub fn push(&mut self, packet: Packet, arrival: Instant);
    pub fn pop_ready(&mut self, now: Instant) -> Option<JitterOutput>;
}
```

`pop_ready` (jitter.rs:91-113) returns an in-order packet **immediately** if it
is present; it only returns `Lost { .. }` once the oldest buffered packet's
*arrival age* reaches `max_depth_ms`. `target_depth` is advisory only and never
delays a pop — so ordered delivery incurs no added latency.

### `FecController` API (`fec.rs`), do not change

```rust
pub struct FecSetting { pub enable: bool, pub packet_loss_perc: u8 }
impl FecController {
    pub fn new(mode: FecMode, on_pct: u32, off_pct: u32, hysteresis_secs: u32) -> Self;
    pub fn record(&mut self, now: Instant, lost: bool);
    pub fn evaluate(&mut self, now: Instant) -> FecSetting;
}
```

### Codec API (`codec.rs`), do not change

```rust
impl OpusEncoder { pub fn set_fec(&mut self, enable: bool, packet_loss_perc: u8) -> Result<(), CodecError>; }
impl OpusDecoder {
    pub fn decode(&mut self, input: Option<&[u8]>, out: &mut [f32]) -> Result<(), CodecError>;
    pub fn decode_with_fec(&mut self, input: Option<&[u8]>, out: &mut [f32], use_fec: bool) -> Result<(), CodecError>;
}
```

### Config defaults (from `settings.rs:57-74`)

The pump signatures must NOT change (see "Design decision"), so the pumps
construct the jitter/FEC config internally using the same defaults the rest of
the app uses:

- `JitterMode::Auto`, `jitter_max_depth_ms = 200`
- `FecMode::Always`, on-threshold `1`, off-threshold `0`, hysteresis `10s`
  (identical to `FecController::new(core_fec_mode, 1, 0, 10)` in `send.rs:33`)

Imports available in the crate: `crate::settings::{JitterMode, FecMode}`,
`crate::net::jitter::{JitterBuffer, JitterOutput}`,
`crate::net::fec::FecController`. `FEC_REEVAL_FRAMES` in the CLI is `100`
(`audio_pipeline.rs:14`); reuse the value as a local `const` (do not import
across crates).

### SAFETY confirmation (docs/SAFETY.md + docs/SPEC.md §5.3)

The two pumps run inside `tokio::spawn` async tasks — **not** cpal callbacks.
The cpal realtime callbacks live only in `audio/capture.rs` and
`audio/playback.rs`, which this plan does NOT touch. Therefore SAFETY invariants
1–3 (no alloc / no lock / no log inside the RT callback) do not constrain the
pump code, and invariant 4 (playback underrun → zero-fill) is enforced inside
`playback.rs`, unchanged. Keep `FRAME_STEREO_SAMPLES`-sized decode buffers.
SPEC §5.3 explicitly places the jitter buffer between "UDP recv" and "Opus
decoder" — this plan makes the real session path match that diagram.

## Design decision (read before writing code)

1. **Do NOT change the signatures of `spawn_source_pump_inner` or
   `spawn_sink_pump_inner`.** They are called from FIVE call sites outside this
   file — `crates/splitter-core/tests/stream_data_plane.rs`,
   `crates/splitter-integration-tests/tests/audio_rms_sustained.rs`,
   `crates/splitter-integration-tests/tests/soak.rs` — plus the in-file
   `open_stream_as_*` orchestration. Changing the signature ripples across
   three crates and is out of scope. Construct the `JitterBuffer` /
   `FecController` **inside** the pump bodies from the defaults above. Adding a
   local `const` and a short `// WHY` comment (project rule: WHY-only comments
   allowed) explaining "defaults mirror Settings until a follow-up threads
   SettingsHandle through open_stream_as_*" is acceptable and expected.

2. **Sink pump uses push-then-drain, NOT a periodic tick.** Mirror `recv.rs`
   exactly: on each received packet, `jitter.push(pkt, now)` then
   `while let Some(out) = jitter.pop_ready(now)`. Do NOT add a 20 ms
   `tokio::time::interval` branch to the `select!`. A tick would change output
   pacing and risk latency — if you believe push-drain is insufficient, that is
   a STOP-and-report condition, not a thing to improvise.

## Commands you will need

| Purpose   | Command                                                        | Expected on success |
|-----------|---------------------------------------------------------------|---------------------|
| Build     | `cargo build --workspace`                                     | exit 0              |
| Tests     | `cargo test --workspace`                                      | all pass            |
| Focused   | `cargo test -p splitter-core --test stream_data_plane`        | all pass            |
| Pump unit | `cargo test -p splitter-core sink_pump`                       | all pass            |
| Lint      | `cargo clippy --workspace --all-targets -- -D warnings`       | exit 0, no warnings |
| Format    | `cargo fmt --all -- --check`                                  | exit 0, no diff     |

## Scope

**In scope** (the only files you should modify):
- `crates/splitter-core/src/net/stream_runtime.rs` — rewrite the bodies of the
  two pumps + update the in-file `sink_pump_records_lost_packets_on_seq_gap`
  test (semantics change — see Step 4).
- `crates/splitter-core/tests/stream_data_plane.rs` — ADD one reorder test.

**Out of scope** (do NOT touch):
- `jitter.rs`, `fec.rs`, `codec.rs` — call their APIs only; they are tested.
- `recv.rs`, `send.rs`, `audio_pipeline.rs` — reference wiring only.
- The `open_stream_as_*` orchestration functions' signatures, and the pump
  function signatures — keep them byte-for-byte identical.
- `crates/splitter-integration-tests/*` — must keep compiling & passing
  unchanged; if a change there seems required, STOP.
- Any change to the on-wire `Packet` format.

## Git workflow

- Branch: `advisor/016-wire-jitterbuffer-and-fec`
- Conventional-commit **title only**, no body, e.g.
  `fix(net): route session sink pump through jitter buffer + FEC`.
- **NEVER** add a `Co-Authored-By` trailer.
- Do NOT push or open a PR.

## Steps

### Step 1: Rewrite the sink pump to use the jitter buffer + FEC recovery

In `spawn_sink_pump_inner` (`stream_runtime.rs:629-704`):

- Remove `let mut last_seq: Option<u32> = None;` and the entire
  `if let Some(prev) = last_seq { ... } last_seq = Some(pkt.seq);` block
  (lines 682–696) — the jitter buffer now owns ordering and loss detection.
- After constructing the decoder, add:
  ```rust
  // WHY: defaults mirror Settings::default(); a follow-up plan threads the
  // real SettingsHandle through open_stream_as_* into the pump.
  const MAX_DEPTH_MS: u32 = 200;
  let mut jitter = JitterBuffer::new(JitterMode::Auto, MAX_DEPTH_MS);
  let mut pending_fec_recover = false;
  ```
- Keep the existing `stats.packets_received` / `bytes_received` /
  `last_seq_received` updates on raw arrival (before pushing to the jitter
  buffer), so byte/packet accounting is unchanged.
- Replace the decode logic with push-then-drain (mirror `recv.rs:42-83`):
  ```rust
  let now = std::time::Instant::now();
  jitter.push(pkt, now);
  while let Some(out) = jitter.pop_ready(now) {
      match out {
          JitterOutput::Lost { .. } => {
              pending_fec_recover = true;
              stats.packets_lost.fetch_add(1, Ordering::Relaxed);
          }
          JitterOutput::Packet(p) => {
              if pending_fec_recover {
                  if decoder.decode_with_fec(Some(&p.payload[..]), &mut decoded, true).is_ok() {
                      apply_gain_and_push(&mut decoded, gain, muted.load(Ordering::Relaxed), paused, &mut producer);
                  }
                  pending_fec_recover = false;
              }
              if decoder.decode_with_fec(Some(&p.payload[..]), &mut decoded, false).is_ok() {
                  apply_gain_and_push(&mut decoded, gain, muted.load(Ordering::Relaxed), paused, &mut producer);
              }
          }
      }
  }
  ```
- Add the imports at the top of the file:
  `use crate::net::jitter::{JitterBuffer, JitterOutput};` and
  `use crate::settings::{FecMode, JitterMode};` (FecMode is needed in Step 2).
  `crate::net::fec::FecController` too.

**Verify**: `cargo build -p splitter-core` → exit 0.

### Step 2: Wire FEC into the source pump

In `spawn_source_pump_inner` (`stream_runtime.rs:553-627`), after the encoder
is constructed (line 570):
```rust
// WHY: default FecMode mirrors Settings::default() (=Always). The source has
// no live packet-loss feedback in the P2P path, so Auto would never flip on;
// evaluating here activates negotiated in-band FEC. Loss-feedback wiring is
// deferred (see Maintenance notes).
const FEC_REEVAL_FRAMES: u32 = 100;
let mut fec = FecController::new(FecMode::Always, 1, 0, 10);
let mut frame_count: u32 = 0;
{
    let setting = fec.evaluate(std::time::Instant::now());
    if let Err(e) = encoder.set_fec(setting.enable, setting.packet_loss_perc) {
        tracing::warn!("initial set_fec failed: {e}");
    }
}
```
Then, inside the frame-ready branch, right after a frame is popped and before
`encoder.encode(...)`, increment and periodically re-evaluate:
```rust
frame_count = frame_count.wrapping_add(1);
if frame_count.is_multiple_of(FEC_REEVAL_FRAMES) {
    let setting = fec.evaluate(std::time::Instant::now());
    if let Err(e) = encoder.set_fec(setting.enable, setting.packet_loss_perc) {
        tracing::warn!("set_fec failed: {e}");
    }
}
```
Do not change the function signature, the `seq`/`SEQ_MASK` handling, or the
send logic.

**Verify**: `cargo build -p splitter-core` → exit 0.

### Step 3: Confirm `seq_gap` / `SEQ_MASK` are still used and warning-free

The sink no longer uses `seq_gap` or `last_seq`, but `seq_gap`/`SEQ_MASK` are
still used by the source pump (`seq & SEQ_MASK`) and by the `seq_gap` unit
tests (`tests` mod, lines 450-467). Do not delete them.

**Verify**: `cargo clippy -p splitter-core --all-targets -- -D warnings` → exit
0 (no `dead_code` warning for `seq_gap`/`SEQ_MASK`). If clippy reports either as
newly unused, re-check that the source pump still masks `seq`.

### Step 4: Update the in-file loss-counting test

The test `sink_pump_records_lost_packets_on_seq_gap`
(`stream_runtime.rs:784-831`) currently sends seq `0` then `3` and asserts
`packets_lost == 2` immediately, because the OLD pump counted PLC skips the
instant the gap was seen. With the jitter buffer, a gap is only declared `Lost`
once the missing slot ages past `MAX_DEPTH_MS` (200 ms) AND a later `pop_ready`
runs. Rewrite the test to match the new, correct semantics:

- Send seq `0` (drains immediately), then seq `2` (leaves seq `1` missing).
- Sleep `> MAX_DEPTH_MS` (e.g. 250 ms).
- Send one more packet (seq `3`) to trigger a `pop_ready` pass at a `now` where
  seq `1`'s successor has aged out.
- Assert `stats.packets_lost.load(Relaxed) >= 1`.

Keep the test name (or rename to `sink_pump_records_lost_packet_after_max_depth`
— your choice, but if you rename, grep to confirm nothing references the old
name). The other sink test, `sink_pump_decodes_into_playback_ring`
(lines 735-782), sends a single in-order packet and must still pass unchanged.

**Verify**: `cargo test -p splitter-core sink_pump` → all pass.

### Step 5: Add a reorder data-plane test

In `crates/splitter-core/tests/stream_data_plane.rs`, add a `#[tokio::test]`
modeled on `volume_change_attenuates_decoded_signal` (lines 86-139), which
already encodes an Opus frame, wraps it in a `Packet`, sends it over UDP to a
`spawn_sink_pump_inner`, and reads the playback ring.

The new test proves out-of-order arrival is reordered before decode:

- Encode THREE distinguishable frames — use three constant DC levels (e.g.
  amplitudes `0.05`, `0.15`, `0.30`) so each decoded frame has a distinct,
  monotonically increasing energy. Wrap them as `Packet { seq: 0/1/2, .. }`.
- Bind a `sink_socket`, spawn `spawn_sink_pump_inner`, and send the packets
  **out of order**: wire order `seq=2`, then `seq=0`, then `seq=1`, each as a
  separate `send_to`, with a short `sleep` between sends so they arrive as
  distinct datagrams.
- Drain the playback ring into three `FRAME_STEREO_SAMPLES` chunks and compute
  each chunk's energy. Assert energies are non-decreasing (`e0 <= e1 <= e2`),
  which can only hold if the frames were emitted in seq order 0,1,2 despite
  arriving 2,0,1.
- Use `JitterMode::Auto` behavior implicitly (the pump constructs it); allow up
  to ~1 s of polling for the ring to fill, like the existing tests.

Name it e.g. `out_of_order_packets_are_reordered_before_decode`.

**Verify**: `cargo test -p splitter-core --test stream_data_plane` → all pass,
including the new test.

### Step 6: Full verification

Run the whole workspace, including the integration-test crate (which calls the
pumps with their unchanged signatures and must stay green):

**Verify**:
- `cargo test --workspace` → all pass.
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0.
- `cargo fmt --all -- --check` → exit 0.

## Test plan

- **New test** in `stream_data_plane.rs`:
  `out_of_order_packets_are_reordered_before_decode` — the regression this plan
  fixes (reordering). Structural model: `volume_change_attenuates_decoded_signal`.
- **Updated test** in `stream_runtime.rs`: loss counting now reflects
  jitter-buffer age-out, not immediate seq-gap PLC.
- **Unchanged, must still pass**: `sink_pump_decodes_into_playback_ring`,
  `source_pump_*`, `pcm_round_trip_source_to_sink_over_localhost_udp`
  (stream_data_plane), and both integration-test crates.
- Verification: `cargo test --workspace` → all pass.

## Done criteria

Machine-checkable. ALL must hold:

- [ ] `cargo build --workspace` exits 0
- [ ] `cargo test --workspace` exits 0; the new reorder test exists and passes
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` exits 0
- [ ] `cargo fmt --all -- --check` exits 0
- [ ] `grep -n "last_seq" crates/splitter-core/src/net/stream_runtime.rs`
      returns only `last_seq_received` (the stats field) — the local
      `last_seq` cursor is gone
- [ ] `grep -n "JitterBuffer\|decode_with_fec\|set_fec" crates/splitter-core/src/net/stream_runtime.rs`
      shows the jitter buffer, FEC decode, and encoder FEC are now wired
- [ ] `git diff --stat 217a31d..HEAD` shows only the two in-scope files changed
- [ ] `plans/README.md` status row updated (if that file exists)

## STOP conditions

Stop and report back (do not improvise) if:

- The "Current state" excerpts don't match the live code (codebase drifted).
- Wiring the jitter buffer **materially raises steady-state latency** on the
  ordered-delivery path (it should not — `pop_ready` returns in-order packets
  immediately). Measure via the round-trip test; if ordered frames now lag,
  report before proceeding.
- You conclude the sink pump needs a periodic timer tick (rather than
  push-then-drain) to work correctly — report; do not add the tick yourself.
- Making this work appears to require changing a pump function signature or any
  `open_stream_as_*` signature (which would ripple into `src-tauri`,
  `splitter-cli`, or `splitter-integration-tests`).
- You discover the pump actually runs inside a cpal RT callback (it must not —
  it runs in a `tokio::spawn` task). If that assumption is false, STOP.
- Any integration-test in `splitter-integration-tests` fails or needs editing.

## Maintenance notes

- **Plan 017 depends on this.** Plan 017 splits `stream_runtime.rs` into sibling
  modules and MOVES these two pump bodies verbatim. Execute 016 first, then
  rebase 017 on top so it moves the already-wired pumps. If 017 was somehow
  started first, it must be re-based to include this wiring.
- **Deferred follow-up (intentional):** the pumps currently hardcode
  `JitterMode::Auto` / 200 ms / `FecMode::Always` because threading the real
  `SettingsHandle` through the `open_stream_as_*` orchestration would change
  their signatures and ripple across `src-tauri` and `splitter-cli`. A future
  plan should add a small config struct parameter to the orchestration
  functions (not the pump `_inner` signatures if avoidable) and pass live
  `Settings`.
- **Deferred follow-up:** the source-side `FecController` has no live
  packet-loss feedback in the P2P path (loss is only observed at the sink), so
  `FecMode::Auto` would never flip on. A future plan should feed sink-observed
  loss back to the source (e.g. via the heartbeat/stats channel) so Auto works.
- **Reviewer scrutiny:** confirm the FEC-recovery double-decode in the sink
  matches `recv.rs::handle_packet` exactly (recover prior frame with
  `use_fec=true`, then decode current frame with `use_fec=false`), and that
  `pending_fec_recover` is cleared on every path.
</content>
</invoke>
