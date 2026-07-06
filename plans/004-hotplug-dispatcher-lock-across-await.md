# Plan 004: Stop holding the stream-registry read lock across control-channel sends in hot-plug dispatch

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat 217a31d..HEAD -- crates/splitter-core/src/net/stream_runtime.rs`
> If the in-scope file changed since this plan was written, compare the
> "Current state" excerpt against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Priority**: P1
- **Effort**: S
- **Risk**: LOW
- **Depends on**: none
- **Category**: perf
- **Planned at**: commit `217a31d`, 2026-07-05

## Why this matters

`dispatch_device_events` is the single task that reacts to audio-device
hot-plug events (a USB headset disappearing/reappearing) by pausing or resuming
every stream bound to that device. It takes a **read** lock on
`StreamRegistry.inner` and then, *while still holding that lock*, awaits
`rt.control_tx.send(signal).await` for each matching stream inside a `for` loop.

`control_tx` is a bounded `tokio::sync::mpsc::Sender` with **capacity 8** (see
the four production channel constructions at `stream_runtime.rs:988, 1023, 1106,
1151`). A bounded `send().await` **suspends** when the channel is full. If a
pump task is momentarily not draining its control channel (busy, blocked on I/O,
or slow), the dispatcher parks *inside* the read lock. Every concurrent
`StreamRegistry::register` and `StreamRegistry::close` needs the **write** lock
(`stream_runtime.rs:227` and `271`), so a single stalled control channel stalls
all stream registration and teardown across the whole process until that one
pump drains. On a device hot-plug that touches several streams this is a real
head-of-line-blocking hazard, not a theoretical one.

The fix is the standard "snapshot under the lock, act outside it" pattern:
collect the `(control_tx.clone(), signal, sid, stream_id)` targets while holding
the read lock, drop the guard, then perform the awaited sends. `control_tx` is
an `mpsc::Sender` and is cheaply `Clone`; `StreamControlSignal` derives `Copy`
(`stream_runtime.rs:58`), so the snapshot is cheap.

## Current state

- `crates/splitter-core/src/net/stream_runtime.rs` — hosts the `StreamRegistry`,
  `StreamRuntime`, `StreamControlSignal`, and the `dispatch_device_events`
  free function.

The function to fix, `crates/splitter-core/src/net/stream_runtime.rs:1427-1458`:

```rust
pub async fn dispatch_device_events(
    registry: Arc<StreamRegistry>,
    mut rx: broadcast::Receiver<DeviceEvent>,
) {
    loop {
        match rx.recv().await {
            Ok(ev) => {
                let (target_id, signal) = match ev {
                    DeviceEvent::Disappeared(id) => (id, StreamControlSignal::Pause),
                    DeviceEvent::Appeared(id) => (id, StreamControlSignal::Resume),
                };
                let guard = registry.inner.read().await;
                for ((sid, stream_id), rt) in guard.iter() {
                    if rt.bound_device_id.as_deref() == Some(target_id.as_str()) {
                        let _ = rt.control_tx.send(signal).await;
                        tracing::info!(
                            session = %sid,
                            stream = %stream_id,
                            device = %target_id,
                            signal = ?signal,
                            "device hot-plug -> pump notified"
                        );
                    }
                }
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!("device watcher lagged by {n} events");
            }
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}
```

Supporting facts (confirm before editing):

- `StreamRegistry.inner` is `pub(crate) RwLock<HashMap<(SessionId, StreamId), StreamRuntime>>`
  (`stream_runtime.rs:216`).
- `StreamRuntime.control_tx` is `pub control_tx: mpsc::Sender<StreamControlSignal>`
  (`stream_runtime.rs:119`).
- `StreamControlSignal` is `#[derive(Debug, Clone, Copy, PartialEq)]`
  (`stream_runtime.rs:58-59`) — `Copy`, so no clone needed for the signal.
- Production control channels are capacity 8: `stream_runtime.rs:988, 1023, 1106, 1151`.
- `SessionId` and `StreamId` in the map key both derive `Copy`/`Clone` (they are
  the newtypes used throughout the file, e.g. `StreamId(0)` at line 1477 and
  `SessionId::new()` at line 1470), so they can be captured into the snapshot by
  value.
- Existing hot-plug test module: `#[cfg(test)] mod hotplug_tests` at
  `stream_runtime.rs:1460-1498`, with the test
  `watcher_dispatches_pause_when_bound_device_disappears` (line 1466). Use it as
  the structural model for the new test.

Repo conventions that apply here:

- **No code comments** except a non-obvious WHY. A single-line WHY explaining
  that the sends happen *after* dropping the read lock (to avoid blocking
  register/close) is warranted and encouraged.
- Conventional-commit **title only**, no body. NEVER add a `Co-Authored-By` trailer.

## Commands you will need

| Purpose   | Command                                                        | Expected on success        |
|-----------|---------------------------------------------------------------|----------------------------|
| Build     | `cargo build -p splitter-core`                               | exit 0                     |
| Tests     | `cargo test -p splitter-core`                                | all pass                   |
| Workspace | `cargo test --workspace`                                     | all pass                   |
| Lint      | `cargo clippy --workspace --all-targets -- -D warnings`      | exit 0, no warnings        |
| Format    | `cargo fmt --all -- --check`                                 | exit 0, no diff            |

## Scope

**In scope** (the only file you should modify):
- `crates/splitter-core/src/net/stream_runtime.rs`

**Out of scope** (do NOT touch, even though they look related):
- The channel capacities at lines 988/1023/1106/1151 — do NOT bump them to
  "fix" the blocking; the lock-scope fix is the correct one and raising capacity
  only hides the problem.
- `StreamRegistry::register` / `close` / `send_control` — the read/write lock
  users the dispatcher was blocking; they are correct and stay unchanged.
- `crates/splitter-cli/src/commands/daemon/mod.rs` and `src-tauri/src/core.rs`,
  which each `tokio::spawn(dispatch_device_events(..))` — the function signature
  must stay identical so these call sites keep compiling untouched.

## Git workflow

- Branch: `advisor/004-hotplug-dispatcher-lock-across-await`
- One commit; conventional-commit title only, e.g.
  `perf(core): release stream-registry read lock before hot-plug control sends`
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Snapshot targets under the read lock, then send after dropping it

Rewrite only the `Ok(ev) => { .. }` arm of `dispatch_device_events`. Keep the
signature, the `Lagged` arm, and the `Closed` arm exactly as they are. Target
shape:

```rust
            Ok(ev) => {
                let (target_id, signal) = match ev {
                    DeviceEvent::Disappeared(id) => (id, StreamControlSignal::Pause),
                    DeviceEvent::Appeared(id) => (id, StreamControlSignal::Resume),
                };
                let targets: Vec<(SessionId, StreamId, mpsc::Sender<StreamControlSignal>)> = {
                    let guard = registry.inner.read().await;
                    guard
                        .iter()
                        .filter(|(_, rt)| {
                            rt.bound_device_id.as_deref() == Some(target_id.as_str())
                        })
                        .map(|((sid, stream_id), rt)| (*sid, *stream_id, rt.control_tx.clone()))
                        .collect()
                };
                // Sends happen after the read guard is dropped so a full control
                // channel cannot block concurrent register/close (write lock).
                for (sid, stream_id, control_tx) in targets {
                    let _ = control_tx.send(signal).await;
                    tracing::info!(
                        session = %sid,
                        stream = %stream_id,
                        device = %target_id,
                        signal = ?signal,
                        "device hot-plug -> pump notified"
                    );
                }
            }
```

Notes:
- The inner block scope (`{ let guard = ...; ... .collect() }`) guarantees the
  `RwLockReadGuard` is dropped before the send loop — that is the whole point.
- `mpsc` is already imported in this file (used throughout, e.g. line 191); if
  the `Vec` type annotation triggers an unresolved-path error, it is because
  `mpsc` must be referenced as it already is elsewhere in the file — check the
  existing `use` at the top of the module and match it (do not add a new
  duplicate import).
- Do NOT change the `tracing::info!` fields — keep the log output identical.

**Verify**: `cargo build -p splitter-core` → exit 0.

### Step 2: Extend the hot-plug test to assert delivery still happens

The existing test `watcher_dispatches_pause_when_bound_device_disappears`
(`stream_runtime.rs:1466`) spawns the dispatcher, sends a `Disappeared` event,
sleeps, and aborts — but it never actually asserts that the bound stream's pump
received the `Pause` signal (it drains `ctrl_rx` in a spawned task and discards
it). Add a **new** test in the same `#[cfg(test)] mod hotplug_tests` module that
asserts the control signal is delivered after a hot-plug event, proving the
new lock-drop-then-send path still delivers.

```rust
    #[tokio::test]
    async fn dispatch_delivers_pause_to_bound_stream_after_lock_release() {
        let registry = StreamRegistry::new();
        let (tx, _) = broadcast::channel::<DeviceEvent>(8);
        let sid = SessionId::new();

        let (ctrl_tx, mut ctrl_rx) = mpsc::channel::<StreamControlSignal>(4);
        let join = tokio::spawn(async {});
        registry
            .register(StreamRuntime {
                session_id: sid,
                stream_id: StreamId(0),
                stats: Arc::new(StreamStats::default()),
                control_tx: ctrl_tx,
                bound_device_id: Some("Input:0:USB Headset".into()),
                join,
                device_guard: DeviceGuard::None,
            })
            .await
            .unwrap();

        let dispatcher = tokio::spawn(dispatch_device_events(registry.clone(), tx.subscribe()));
        tx.send(DeviceEvent::Disappeared("Input:0:USB Headset".into()))
            .unwrap();

        let received = tokio::time::timeout(std::time::Duration::from_secs(1), ctrl_rx.recv())
            .await
            .expect("control signal should arrive within 1s");
        assert_eq!(received, Some(StreamControlSignal::Pause));

        dispatcher.abort();
    }
```

If the pre-existing `watcher_dispatches_pause_when_bound_device_disappears` test
now overlaps this one, leave it as-is (do not delete it) unless clippy/compiler
forces a change; adding coverage is the goal, not removing it.

**Verify**: `cargo test -p splitter-core dispatch_delivers_pause_to_bound_stream_after_lock_release`
→ 1 test passes.

### Step 3: Full verification

**Verify**:
- `cargo test --workspace` → all pass
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0

## Test plan

- New test in `crates/splitter-core/src/net/stream_runtime.rs`,
  `#[cfg(test)] mod hotplug_tests`:
  `dispatch_delivers_pause_to_bound_stream_after_lock_release` — asserts a bound
  stream's control channel actually receives `Pause` after a `Disappeared`
  event (regression that the lock-drop-then-send path still delivers).
- Structural pattern: the existing
  `watcher_dispatches_pause_when_bound_device_disappears` test in the same module.
- The "does not block register/close" property is guaranteed structurally by the
  guard being dropped before the send loop (verified by inspection + the scoped
  block); a timing test that forces channel backpressure is intentionally not
  added because it would be flaky.
- Verification: `cargo test --workspace` → all pass, including the 1 new test.

## Done criteria

Machine-checkable. ALL must hold:

- [ ] `cargo build -p splitter-core` exits 0
- [ ] `cargo test --workspace` exits 0; `dispatch_delivers_pause_to_bound_stream_after_lock_release` exists and passes
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` exits 0
- [ ] `cargo fmt --all -- --check` exits 0
- [ ] In `dispatch_device_events`, no `.send(signal).await` occurs while a
      `registry.inner.read().await` guard is in scope (verify by reading the
      function: the guard lives only inside the `{ .. .collect() }` block).
- [ ] The `dispatch_device_events` signature is unchanged
      (`grep -n "pub async fn dispatch_device_events" crates/splitter-core/src/net/stream_runtime.rs`
      still shows the same two-arg signature).
- [ ] No files outside `crates/splitter-core/src/net/stream_runtime.rs` are modified (`git status`)
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- The `dispatch_device_events` body no longer matches the "Current state"
  excerpt (it was already refactored).
- Capturing `*sid`/`*stream_id` fails to compile because `SessionId`/`StreamId`
  are not `Copy` in the live code — that contradicts a stated assumption; report
  it rather than adding `.clone()` blindly.
- Any verification command fails twice after a reasonable fix attempt.
- The fix appears to require changing `dispatch_device_events`'s signature or any
  out-of-scope file.

## Maintenance notes

For the human/agent who owns this code after the change lands:

- The invariant to preserve: **never `.await` a bounded channel send (or any
  potentially-parking await) while holding `registry.inner`'s read or write
  lock.** If future work adds more per-stream fan-out here, snapshot first.
- A reviewer should confirm the `RwLockReadGuard` is dropped before the send
  loop (the scoped `{ .. }` block) and that the `tracing::info!` output is
  unchanged.
- Deferred out of scope: auditing other lock-across-await sites in the file (e.g.
  `send_control` at line 250 holds only a read lock for a single send; assess
  separately if it ever becomes a hot path).
