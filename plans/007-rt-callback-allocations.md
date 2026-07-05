# Plan 007: Pre-size all cpal / SCK callback buffers so real-time callbacks never heap-allocate

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat 217a31d..HEAD -- crates/splitter-core/src/audio/capture.rs crates/splitter-core/src/audio/playback.rs crates/splitter-core/src/audio/loopback/macos.rs`
> If any in-scope file changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Priority**: P1
- **Effort**: S
- **Risk**: MED
- **Depends on**: none
- **Category**: perf
- **Planned at**: commit `217a31d`, 2026-07-05

## Why this matters

`docs/SAFETY.md` invariant **#1 ("No allocation in callbacks")** is a hard rule for
the cpal audio callbacks: *"No `Box::new`, no `Vec::push` that grows … All buffers
must be pre-allocated at stream construction time."* Three real-time callbacks
currently violate it on large device buffers:

1. **Capture fast path** (`capture.rs`) grows `scratch` per callback; it was built
   with capacity `1024`, so any callback delivering **> 512 stereo frames**
   reallocates on the audio thread.
2. **Playback** (`playback.rs`) `resize`s `stereo_buf` (cap `4096`) per callback —
   reallocs above **2048 stereo frames** — and `extend`s `reservoir` (cap `4096`)
   which can grow past its capacity.
3. **macOS loopback** (`loopback/macos.rs`) allocates a **fresh `Vec::new()` every
   delivery callback** and grows it via deinterleave.

A heap allocation inside an audio callback can block on the allocator lock and
cause an audible glitch (xrun). Device buffer sizes are chosen by the OS/driver
and can legitimately be 1024–4096 frames (see the `SupportedBufferSize::Range { min: 128, max: 4096 }`
already hard-coded in the WASAPI fallback, `capture.rs:118-122`), so these paths
*do* get exercised in the field. The fix pre-reserves the maximum possible
callback length once at stream construction and turns the per-callback grow
operations into non-allocating length changes guarded by `debug_assert!`.

### SAFETY.md invariants this plan MUST preserve (quote them in the PR)

From `docs/SAFETY.md`:

- **#1 No allocation in callbacks.** All buffers pre-allocated at construction.
- **#2 No blocking in callbacks.** No `Mutex::lock` (blocking), no `tokio::sync::*`,
  no blocking syscalls. Communication is via the SPSC ring only; the existing code
  uses `try_lock()` (non-blocking) — you MUST keep using `try_lock`, never `lock`.
- **#4 Underrun in playback → zero-fill.** When the ring returns fewer samples than
  needed, fill the remainder with `0.0f32`. Do not change this behavior.
- **#7 Frame size fixed** `FRAME_SAMPLES = 960`. Do not touch.

**You are adding capacity reservation and `debug_assert!`s only. You must NOT add
any lock, any `.lock()`, any logging (`tracing::*`), any `format!`, or any new
allocation inside a callback body.** A reviewer will diff for exactly that.

## Current state

### 1. Capture — `crates/splitter-core/src/audio/capture.rs`

`SampleRouter` is constructed in `SampleRouter::new(sample_rate, channels)`
(capture.rs:316-342). It does **not** currently receive the device buffer size.
Its scratch is sized by a constant:

```rust
let scratch_cap = if resampler_l.is_some() {
    RESAMPLE_CHUNK * 4
} else {
    1024
};

Ok(Self {
    channels: channels as usize,
    resampler_l,
    resampler_r,
    scratch: Vec::with_capacity(scratch_cap),
    resampled: Vec::with_capacity(scratch_cap * 2),
    l_in: Vec::with_capacity(RESAMPLE_CHUNK),
    r_in: Vec::with_capacity(RESAMPLE_CHUNK),
    l_out: Vec::with_capacity(RESAMPLE_CHUNK * 2),
    r_out: Vec::with_capacity(RESAMPLE_CHUNK * 2),
})
```

The offending fast path (`convert_and_route`, capture.rs:366-376):

```rust
if self.resampler_l.is_none() {
    self.scratch.clear();
    self.scratch.reserve(frame_count * 2);   // <-- reallocs when frame_count*2 > capacity
    for i in 0..frame_count {
        let (l, r) = deinterleave_stereo_frame(interleaved, i * ch, ch, &to_f32);
        self.scratch.push(l);
        self.scratch.push(r);
    }
    flush_to_ring(&self.scratch, prod, notify);
    return;
}
```

`SampleRouter::new` is called from `build_capture_stream` (capture.rs:184, 201,
218), which receives `supported: cpal::SupportedStreamConfig` and does
`let config: cpal::StreamConfig = supported.into();` at capture.rs:174 — so
`supported.buffer_size()` is available **before** that `into()` and can supply the
max frame count.

### 2. Playback — `crates/splitter-core/src/audio/playback.rs`

`PlaybackFiller::new(device_rate, channels)` (playback.rs:172-202) sizes:

```rust
reservoir: Vec::with_capacity(4096),
src_scratch: Vec::with_capacity(RESAMPLE_CHUNK * 8),
l_in: Vec::with_capacity(RESAMPLE_CHUNK),
r_in: Vec::with_capacity(RESAMPLE_CHUNK),
l_out: Vec::with_capacity(RESAMPLE_CHUNK * 2),
r_out: Vec::with_capacity(RESAMPLE_CHUNK * 2),
stereo_buf: Vec::with_capacity(4096),
```

The offending `produce_stereo` (playback.rs:250-321):

```rust
fn produce_stereo(&mut self, frames: usize, cons: &Mutex<RingConsumer>, notify: &Notify) {
    let stereo_needed = frames * 2;
    self.stereo_buf.clear();
    self.stereo_buf.resize(stereo_needed, 0.0);   // <-- reallocs when stereo_needed > capacity
    ...
    while self.reservoir.len() < stereo_needed {
        ...
        self.reservoir.extend(                    // <-- can grow reservoir past capacity
            self.l_out.iter().zip(self.r_out.iter()).flat_map(|(&lv, &rv)| [lv, rv]),
        );
    }
    let available = self.reservoir.len().min(stereo_needed);
    self.stereo_buf[..available].copy_from_slice(&self.reservoir[..available]);
    self.reservoir.drain(..available);
}
```

`PlaybackFiller::new` is called from `PlaybackHandle::from_device` (playback.rs:57,
73, 91), which has `supported = device.default_output_config()?` (playback.rs:39)
and does `let config: cpal::StreamConfig = supported.into();` at playback.rs:48.
So `supported.buffer_size()` is available before the `into()`.

Note `RESAMPLE_CHUNK = 441` (playback.rs:153). In the resampler path the reservoir
overshoots `stereo_needed` by at most one resampler output block (`l_out`/`r_out`
are each ≤ `RESAMPLE_CHUNK * 2` per channel → ≤ `RESAMPLE_CHUNK * 4` interleaved).

### 3. macOS loopback — `crates/splitter-core/src/audio/loopback/macos.rs`

`AudioHandler` (macos.rs:69-72) holds `producer: Arc<Mutex<RingProducer>>` and
`frame_notify`. The SCK delivery callback `did_output_sample_buffer` takes `&self`
and allocates per call (macos.rs:84):

```rust
let mut stereo: Vec<f32> = Vec::new();       // <-- fresh allocation every callback

for buf in abl.iter() {
    ...
    deinterleave_to_stereo(samples, channels, &mut stereo);   // grows it (macos.rs:45-67)
}

if stereo.is_empty() { return; }

if let Ok(mut p) = self.producer.try_lock() {   // already non-blocking (try_lock)
    let pushed = p.push_slice(&stereo);
    if pushed > 0 { self.frame_notify.notify_one(); }
}
```

`deinterleave_to_stereo` (macos.rs:45-67) itself calls `out.reserve(..)` and
`out.push`/`extend_from_slice` — those are fine **iff** `out` is pre-reserved and
cleared between callbacks so the grows are no-ops. The handler is built in
`start_with_notify` (macos.rs:187-190). SCK delivers audio on a single serialized
thread, so a `Mutex<Vec<f32>>` guarded by `try_lock` (same discipline already used
for `producer`) is a sound place to hold reusable scratch.

### cpal API facts (verified for cpal 0.15.3, `Cargo.lock:910-912`)

- `SupportedStreamConfig::buffer_size(&self) -> &SupportedBufferSize`.
- `enum SupportedBufferSize { Range { min: FrameCount, max: FrameCount }, Unknown }`
  where `FrameCount = u32`. The `Range` variant is already matched in this repo at
  `capture.rs:118-122`.
- `frame_count` in the callbacks is `interleaved.len() / channels` (capture) and
  `out.len() / channels` (playback); the device buffer size is in **frames**, so
  the stereo sample count is `max_frames * 2`.

### Conventions

- **No code comments** except a non-obvious *why* (project rule). A `debug_assert!`
  with a message is allowed and is the preferred "documentation" here.
- Match the existing constructor style (`Vec::with_capacity(..)`), `try_lock`, and
  the `flush_to_ring` / `write_stereo_to_frame` helpers already in these files.

## Commands you will need

| Purpose          | Command                                                                       | Expected on success |
|------------------|-------------------------------------------------------------------------------|---------------------|
| Build            | `cargo build --workspace`                                                     | exit 0              |
| Build (macOS SCK)| `cargo build -p splitter-core --features sck` (only on macOS)                 | exit 0              |
| Tests            | `cargo test -p splitter-core capture`                                         | all pass            |
| Tests            | `cargo test -p splitter-core playback`                                        | all pass            |
| Tests            | `cargo test --workspace`                                                       | all pass            |
| Lint             | `cargo clippy --workspace --all-targets -- -D warnings`                       | exit 0              |
| Format           | `cargo fmt --all -- --check`                                                   | exit 0              |

Note: `loopback/macos.rs` is `#![cfg(all(target_os = "macos", feature = "sck"))]`.
On non-macOS it does not compile at all; do the macOS step's edit but verify it via
`cargo build` reasoning + `cargo fmt`/`clippy` (clippy will still parse it on
macOS). If you are not on macOS, make the edit as specified and mark the macOS-only
build/test as a documented external blocker (see STOP conditions).

## Scope

**In scope** (the only files you should modify):
- `crates/splitter-core/src/audio/capture.rs`
- `crates/splitter-core/src/audio/playback.rs`
- `crates/splitter-core/src/audio/loopback/macos.rs`

**Out of scope** (do NOT touch):
- `crates/splitter-core/src/audio/ring.rs`, `resampler.rs`, `codec.rs` — the ring,
  resampler, and codec are not the source of these allocations.
- The `BufferSize`/`StreamConfig` you pass to `build_input_stream`/
  `build_output_stream` — do **not** switch to `BufferSize::Fixed`. Some hosts
  (CoreAudio, WASAPI shared) ignore or reject it; you are only *reading*
  `buffer_size()` to size your own scratch, not constraining the driver.
- SAFETY.md itself — do not edit the doc.
- The underrun zero-fill logic and the gap/loss logic — behavior must not change.

## Git workflow

- Branch: `advisor/007-rt-callback-allocations`
- Commit style: conventional-commit **title only**, no body. Example fitting title:
  `perf(audio): pre-size rt callback scratch to avoid heap growth`.
- **NEVER** add a `Co-Authored-By` trailer of any kind.
- Do NOT push or open a PR.

## Steps

### Step 0: Add a shared helper to read the max callback frame count

Add a small free function (place it near `RESAMPLE_CHUNK` in **both** `capture.rs`
and `playback.rs`, or add it once in one file and `pub(crate)` — simplest is to add
it privately in each since they are separate modules). It maps a
`SupportedBufferSize` to a concrete max frame count, applying a documented fallback
when the driver reports `Unknown`:

```rust
// WHY: SAFETY.md #1 forbids callback-time allocation, so scratch must be pre-sized
// to the largest buffer the driver can hand us. Drivers that report Unknown give no
// bound; 4096 matches the max already assumed for WASAPI shared mode (see the
// SupportedBufferSize::Range max in start_loopback_wasapi) and is clamped as a
// safety net, not a hard driver limit.
const FALLBACK_MAX_CALLBACK_FRAMES: usize = 4096;

fn max_callback_frames(buffer_size: &cpal::SupportedBufferSize) -> usize {
    match buffer_size {
        cpal::SupportedBufferSize::Range { max, .. } => (*max as usize).max(FALLBACK_MAX_CALLBACK_FRAMES),
        cpal::SupportedBufferSize::Unknown => FALLBACK_MAX_CALLBACK_FRAMES,
    }
}
```

Rationale for `.max(FALLBACK_...)`: a driver may report a `Range` whose `max` it
does not strictly honor under `BufferSize::Default`; taking the larger of the
reported max and the fallback keeps us safe without ever shrinking below a known
value. If a callback ever exceeds this, the `debug_assert!`s in the steps below
fire in test/debug builds; in release the `resize`/`reserve` still behaves
correctly (it would allocate once and never again) — but that must never happen for
in-spec devices, which is what the assert enforces.

**Verify**: `cargo build --workspace` → exit 0 (function may be `#[allow(dead_code)]`
until wired in the next steps; wire it immediately so no dead-code warning remains).

### Step 1: Capture — thread max frames into `SampleRouter::new` and pre-size scratch

1. Change `SampleRouter::new(sample_rate: u32, channels: u16)` to
   `SampleRouter::new(sample_rate: u32, channels: u16, max_frames: usize)`.
2. In `build_capture_stream`, compute `let max_frames = max_callback_frames(supported.buffer_size());`
   **before** `let config: cpal::StreamConfig = supported.into();` (capture.rs:174),
   and pass `max_frames` to each `SampleRouter::new(sample_rate, channels, max_frames)`
   call (capture.rs:184, 201, 218).
3. In `SampleRouter::new`, size the fast-path scratch from `max_frames`:

   ```rust
   let scratch_cap = if resampler_l.is_some() {
       RESAMPLE_CHUNK * 4
   } else {
       max_frames * 2
   };
   ```

   Keep `resampled: Vec::with_capacity(scratch_cap * 2)` (still adequate).
4. In the fast path of `convert_and_route`, replace the per-callback `reserve` with
   a debug assertion (the capacity is now guaranteed):

   ```rust
   if self.resampler_l.is_none() {
       self.scratch.clear();
       debug_assert!(
           frame_count * 2 <= self.scratch.capacity(),
           "capture scratch too small: need {} have {}",
           frame_count * 2,
           self.scratch.capacity()
       );
       for i in 0..frame_count {
           let (l, r) = deinterleave_stereo_frame(interleaved, i * ch, ch, &to_f32);
           self.scratch.push(l);
           self.scratch.push(r);
       }
       flush_to_ring(&self.scratch, prod, notify);
       return;
   }
   ```

   (The `push` calls no longer grow because capacity ≥ `frame_count*2`.)

**Verify**: `cargo test -p splitter-core capture` → all pass, including
`sample_router_fast_path_no_truncation_large_callback` (capture.rs:709, drives 4096
frames).

### Step 2: Playback — thread max frames into `PlaybackFiller::new` and pre-size buffers

1. Change `PlaybackFiller::new(device_rate: u32, channels: u16)` to add a
   `max_frames: usize` parameter.
2. In `PlaybackHandle::from_device`, compute `let max_frames = max_callback_frames(supported.buffer_size());`
   **before** `let config: cpal::StreamConfig = supported.into();` (playback.rs:48),
   and pass it to each `PlaybackFiller::new(sample_rate, channels, max_frames)` call
   (playback.rs:57, 73, 91).
3. In `PlaybackFiller::new`, size the two growable buffers from `max_frames`
   (keep the others):

   ```rust
   reservoir: Vec::with_capacity(max_frames * 2 + RESAMPLE_CHUNK * 4),
   ...
   stereo_buf: Vec::with_capacity(max_frames * 2),
   ```

   `reservoir` needs `stereo_needed` (= `frames*2` ≤ `max_frames*2`) plus one
   resampler-block overshoot (`RESAMPLE_CHUNK * 4` interleaved) — hence the `+`.
4. In `produce_stereo`, keep `clear()` + `resize()` (resize within capacity does not
   allocate) but add a guard so a violation is caught in tests:

   ```rust
   let stereo_needed = frames * 2;
   self.stereo_buf.clear();
   debug_assert!(
       stereo_needed <= self.stereo_buf.capacity(),
       "playback stereo_buf too small: need {} have {}",
       stereo_needed,
       self.stereo_buf.capacity()
   );
   self.stereo_buf.resize(stereo_needed, 0.0);
   ```

   And after the `while` loop, before slicing, assert the reservoir never grew past
   its reservation:

   ```rust
   debug_assert!(
       self.reservoir.len() <= self.reservoir.capacity(),
       "playback reservoir grew past reservation"
   );
   ```

   Do **not** change the underrun zero-fill (`for s in self.stereo_buf[popped..]...`)
   or the drain logic — SAFETY.md #4.

**Verify**: `cargo test -p splitter-core playback` → all pass.

### Step 3: macOS loopback — hold reusable scratch on the handler

Edit `crates/splitter-core/src/audio/loopback/macos.rs`:

1. Add a scratch field to `AudioHandler`, guarded like `producer`:

   ```rust
   struct AudioHandler {
       producer: Arc<Mutex<RingProducer>>,
       frame_notify: Arc<Notify>,
       stereo: Mutex<Vec<f32>>,
   }
   ```

2. Initialize it in `start_with_notify` where the handler is built (macos.rs:187):

   ```rust
   // WHY: SCK delivers audio on a single serialized thread; pre-reserving avoids
   // per-callback allocation (SAFETY.md #1). 8192 stereo frames covers the largest
   // SCK audio buffer observed (see deinterleave large-input tests).
   let handler = AudioHandler {
       producer: Arc::new(Mutex::new(producer)),
       frame_notify: frame_notify.clone(),
       stereo: Mutex::new(Vec::with_capacity(8192 * 2)),
   };
   ```

3. In `did_output_sample_buffer`, replace `let mut stereo: Vec<f32> = Vec::new();`
   with a `try_lock` on the reusable buffer, cleared per callback:

   ```rust
   let Ok(mut stereo) = self.stereo.try_lock() else {
       return;
   };
   stereo.clear();

   for buf in abl.iter() {
       ...
       deinterleave_to_stereo(samples, channels, &mut stereo);
   }

   if stereo.is_empty() {
       return;
   }

   if let Ok(mut p) = self.producer.try_lock() {
       let pushed = p.push_slice(&stereo);
       if pushed > 0 {
           self.frame_notify.notify_one();
       }
   }
   ```

   Keep using `try_lock` (never `lock`) — SAFETY.md #2 spirit. `deinterleave_to_stereo`
   internally `reserve`s/`push`es, but because `stereo` is pre-reserved and cleared,
   these do not allocate for in-spec buffers.

**Verify (macOS only)**: `cargo build -p splitter-core --features sck` → exit 0,
then `cargo test -p splitter-core --features sck loopback::macos` → the
`deinterleave_*_large_input` tests (macos.rs:220-278) pass. If not on macOS, see the
external-blocker note.

### Step 4: Full verification

Run the whole gate:

- `cargo test --workspace`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo fmt --all -- --check`

**Verify**: all three exit 0.

## Test plan

- Existing `capture.rs` test `sample_router_fast_path_no_truncation_large_callback`
  (capture.rs:709) already drives 4096 frames through the fast path; with
  `max_frames = 4096` its scratch is pre-sized, so it passes with the new
  `debug_assert!` active (test builds run debug assertions). Keep it green.
- Add `sample_router_fast_path_no_realloc_within_max` in `capture.rs` tests: build a
  router via `SampleRouter::new(48_000, 2, 4096)`, capture
  `router.scratch.capacity()` (add a `#[cfg(test)]` accessor or read the field
  directly since tests are in the same module), push a 4096-frame stereo callback,
  then assert `router.scratch.capacity()` is unchanged (no reallocation occurred).
- Add `playback_filler_produce_stereo_no_realloc_within_max` in `playback.rs` tests:
  build `PlaybackFiller::new(48_000, 2, 4096)`, record `stereo_buf.capacity()` and
  `reservoir.capacity()`, call `produce_stereo(4096, &cons, &notify)` (with an empty
  ring — underrun path zero-fills), assert both capacities unchanged.
- Existing macOS `deinterleave_*_large_input` tests (8192 frames) already prove the
  deinterleave helper handles large input; they exercise the same code the callback
  now feeds a pre-reserved buffer. Keep them green.
- Verification: `cargo test --workspace` → all pass, new tests included.

## Done criteria

Machine-checkable. ALL must hold:

- [ ] `cargo build --workspace` exits 0
- [ ] `cargo test --workspace` exits 0; the two new no-realloc tests exist and pass
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` exits 0
- [ ] `cargo fmt --all -- --check` exits 0
- [ ] `grep -n "Vec::new()" crates/splitter-core/src/audio/loopback/macos.rs` shows
      no `stereo` allocation inside `did_output_sample_buffer` (the per-callback
      `Vec::new()` at the old line 84 is gone)
- [ ] No new `.lock(` (blocking), `tracing::`, or `format!` added inside any callback
      body (`git diff` review)
- [ ] On macOS only: `cargo build -p splitter-core --features sck` exits 0
- [ ] No files outside the in-scope list are modified (`git status`)
- [ ] `plans/README.md` status row for 007 updated

## STOP conditions

Stop and report back (do not improvise) if:

- The drift check shows any in-scope file changed and the "Current state" excerpts
  no longer match.
- `cpal::SupportedStreamConfig::buffer_size()` or the `SupportedBufferSize` enum
  shape differs from what is described (Cargo.lock shows a cpal version other than
  0.15.x, or the `Range { min, max }` / `Unknown` variants are gone). Sizing the max
  buffer then requires an API you do not have — **report back** rather than guessing
  a bound.
- Any callback body would need a lock, a log, a `format!`, or an allocation to make
  the change work — that means the approach is wrong for that spot; stop and report.
- The `sample_router_fast_path_no_truncation_large_callback` test or any playback
  test fails after the change — do not weaken the test.
- You cannot pre-size a buffer because the max is genuinely unknowable at
  construction for a given path (and the `FALLBACK_MAX_CALLBACK_FRAMES` net feels
  wrong for it) — report the specific path instead of shipping a guess.

## Bloqueios externos

If you are executing on a non-macOS host, the macOS SCK path
(`loopback/macos.rs`, Step 3) cannot be compiled or run here
(`#![cfg(all(target_os = "macos", feature = "sck"))]`). Make the code edit exactly
as specified, run `cargo fmt`/`clippy` (which still format-check/lint the file on
any host via the normal parse), and document in your final report that the
macOS-only build/test (`cargo build -p splitter-core --features sck` and the
`loopback::macos` tests) could not be executed on this platform and must be run on a
macOS machine before merge. This is the only acceptable "did not run" item.

## Maintenance notes

- If a future device or host legitimately delivers callbacks larger than
  `FALLBACK_MAX_CALLBACK_FRAMES` (4096) *and* reports `SupportedBufferSize::Unknown`,
  the `debug_assert!`s will fire in debug/test builds — that is the signal to raise
  the fallback, not to remove the assert.
- The SCK scratch cap (8192 stereo frames) is a heuristic sized from the existing
  large-input tests. If SCK delivery buffers grow (e.g. queue_depth tuning in
  `start_with_notify`), revisit that reservation.
- Reviewer scrutiny: diff every callback body for a lock/log/alloc regression
  (SAFETY.md #1/#2); confirm the underrun zero-fill (SAFETY.md #4) is byte-identical;
  confirm `try_lock` is still used everywhere a `Mutex` is touched from a callback.
- This plan does not constrain the driver buffer size (`BufferSize::Default` stays);
  if a later change switches to `BufferSize::Fixed`, the `max_callback_frames`
  helper should then read the *fixed* size instead of the supported max.
