# Plan 018: Lock the Tauri command layer and acceptor behavior with characterization tests

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**:
> `git diff --stat 217a31d..HEAD -- src-tauri/src/acceptor.rs src-tauri/src/core.rs src-tauri/src/commands/`
> If any in-scope file changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Priority**: P1
- **Effort**: L
- **Risk**: LOW
- **Depends on**: none
- **Category**: tests
- **Planned at**: commit `217a31d`, 2026-07-05

## Why this matters

The Tauri app has ~50 `#[tauri::command]` handlers and a 447-line
`acceptor.rs` that implements the entire trust-gated inbound signaling flow
(session acceptance, stream open-as-sink, stream control, remote-session close,
device-list exchange, peer rename, disconnect teardown, and reconnect
triggering). Today the only tests are enum-conversion / serde tests — none of
the actual *behavior* is covered. `acceptor.rs` in particular has zero tests
despite being the security-sensitive path where a remote peer's messages mutate
local session and stream state.

Plan 020 will hoist this control-plane logic into `splitter-core` and delete
both the Tauri and CLI copies. That refactor is only safe if we first pin down
what the current code *does* — including its quirks — so a regression shows up
as a red test. This plan writes those characterization tests. It is purely
additive: no production behavior changes. Where a handler cannot be exercised
without a Tauri runtime, we extract its pure body into a `_core` helper
(matching the existing `disconnect_all_core` / `open_stream_core` pattern) so it
becomes testable, keeping behavior byte-for-byte identical.

## Current state

### The test harness already available

`AppCore::init(config_dir: &Path)` builds a fully wired core against a temp dir
and binds a real signaling server on an ephemeral port. It is already used in
tests — `src-tauri/src/core.rs:206-212`:

```rust
#[tokio::test]
async fn init_builds_all_handles_in_temp_dir() {
    let dir = tempdir().unwrap();
    let core = AppCore::init(dir.path()).await.expect("init");
    assert!(core.server.bind_addr.port() > 0);
    assert_eq!(core.sessions.snapshot().await.len(), 0);
}
```

`AppCore::emit` is a **no-op when no Tauri `AppHandle` is set** (`core.rs:110-118`:
`if let Some(app) = self.app.get()`), and in `init` the `app` field is
`OnceLock::new()` (unset). So tests can drive code that calls `core.emit(..)`
freely — the emit silently does nothing. This is the key that makes the
acceptor testable without a Tauri runtime.

### The acceptor is driven by a broadcast channel

`spawn_acceptor` (`src-tauri/src/acceptor.rs:24-433`) has this signature:

```rust
pub fn spawn_acceptor(
    core: Arc<AppCore>,
    peer_id: Uuid,
    mut events: tokio::sync::broadcast::Receiver<PeerEvent>,
    addr: SocketAddr,
)
```

`PeerEvent` (`crates/splitter-core/src/net/signaling/connection.rs:16-21`) is
`Clone`:

```rust
pub enum PeerEvent {
    Connected { peer_id: Uuid },
    Message(SignalingMessage),
    Disconnected { reason: String },
}
```

A test can therefore create `tokio::sync::broadcast::channel::<PeerEvent>(16)`,
call `spawn_acceptor(core.clone(), peer_id, rx, addr)`, push
`PeerEvent::Message(..)` through `tx`, yield, and assert on
`core.sessions.snapshot().await`, `core.remote_devices`, `core.peers`, etc.

### Acceptor branches and what each does (the behavior to lock)

- **`SessionRequest { session_id, requested_by }`** (`acceptor.rs:36-105`):
  first **evicts stale sessions** — for every existing session whose
  `remote_peer_id == requester` but whose id differs, closes its streams and the
  session (lines 47-56). Then `register_incoming` + `accept`, resolves a display
  name from trust then discovered peers then the first 8 chars of the uuid,
  emits `IncomingSession` + `SnapshotChanged`. Net observable state: exactly one
  `Active` session for that requester.
- **`StreamControl { stream_id, action }`** (`acceptor.rs:241-297`): finds all
  session ids with `remote_peer_id == peer_id`. `Close` → `stream_registry.close`
  + `sessions.remove_stream`. Non-close → converts to `StreamControlSignal`; if
  `SetMuted(m)`, **also** calls `sessions.set_stream_muted(sid, stream, m)` for
  each session (this is the behavior the CLI is missing — see plan 020), then
  `send_control` for each.
- **`SessionResponse { accepted: false }`** (`acceptor.rs:298-319`): closes the
  named session's streams then the session.
- **`DeviceListResponse { devices }`** (`acceptor.rs:337-340`):
  `core.remote_devices.write().await.insert(peer_id, devices)`.
- **`PeerRenamed { peer_id: rid, peer_name }`** (`acceptor.rs:341-355`): applies
  `crate::core::apply_peer_rename(&mut peers, &rid, &peer_name)`; if it changed,
  emits `PeersChanged`.
- **`Disconnected { reason }`** (`acceptor.rs:391-424`): emits `PeerDisconnected`,
  tears down all sessions+streams for the peer, and **only if there was an active
  session** calls `crate::reconnect::spawn_reconnect(core, peer_id, addr)`, then
  `break`s the loop. `spawn_reconnect` (`reconnect.rs:8-15`) returns immediately
  ("skip reconnect") when the peer is in neither `core.outgoing` nor
  `core.peers` — which is the case in a bare test, so it is a safe no-op.
- **`StreamOpen`** (`acceptor.rs:106-240`), **`DeviceListRequest`**
  (`acceptor.rs:320-336`), **`StreamRequest`** (`acceptor.rs:356-388`): these
  send a reply back to the peer via `send_to_peer` (`acceptor.rs:435-447`), which
  looks the peer up in `core.server.connections` / `core.outgoing`. With no
  connected peer the reply is silently dropped, but the **local state effects
  still happen** and are observable: `StreamOpen` runs `open_stream_as_sink`
  (binds a real ephemeral UDP socket, returns a port), then `add_stream` +
  `activate_stream` on the session. So `StreamOpen` is characterizable for its
  *state* effect (a stream is added to the session) even though the ack itself is
  not observable without a peer connection.

### Command bodies that already delegate to testable `_core` helpers

These take `&AppCore` (not `tauri::State`) and are directly callable in tests:

- `crate::commands::ops::mute_all_core(core)` (`ops.rs:6-26`) — mutes every
  stream: `set_stream_muted(true)` + `send_control(SetMuted(true))`.
- `crate::commands::ops::disconnect_all_core(core)` (`ops.rs:28-35`) — calls
  `teardown_session` for every session.
- `crate::commands::peers::teardown_session(core, sid)` (`peers.rs:177-219`) —
  closes streams, notifies remote (no-op without a connection), sends
  `SessionResponse{accepted:false}` if a tx exists, closes the session.
- `crate::commands::streams::notify_remote(core, sid, stream_id, action)`
  (`streams.rs:20-41`) — no-op-safe when the session or connection is absent.
- `crate::commands::streams::open_stream_core(core, ..)` (`streams.rs:79-183`) —
  requires a live connection to the sink peer (`find_peer_conn` → `Err` "no live
  signaling connection to remote peer"). Its **early-return error paths** are
  characterizable without a peer: session-not-found and
  session-not-bound-to-peer both return `Err` before any connection lookup.

`SessionManager` API used above (`crates/splitter-core/src/net/manager.rs`):
`open_outgoing(local, remote) -> SessionId` (39,72), `register_incoming(id,
local, remote)` (79), `accept(&id)` (95), `add_stream(&id, stream)` (99),
`activate_stream(&id, stream_id)` (103), `close(&id)` (112),
`set_stream_muted(&id, stream_id, bool)` (120), `snapshot() -> Vec<SessionSnapshot>`
(145). A stream value is built with
`splitter_core::net::stream::Stream::new_negotiating(StreamId(n), route, port)`
(see `acceptor.rs:150-154`).

### Conventions to honor

- **No code comments** except a non-obvious WHY (`CLAUDE.md`). Test names carry
  the intent; do not narrate.
- Tests live in an in-file `#[cfg(test)] mod tests { use super::*; .. }` block,
  as in `core.rs:201`, `streams.rs:305`, `peers.rs:229`, `ops.rs` (none yet).
- Async tests use `#[tokio::test]`. Temp dirs use `tempfile::tempdir()`
  (already a dev-dependency — see `core.rs:204`).
- Conventional-commit **title only**, no body. **Never** add a `Co-Authored-By`
  trailer.

## Commands you will need

| Purpose   | Command                                                         | Expected on success |
|-----------|----------------------------------------------------------------|---------------------|
| Build     | `cargo build -p splitter`                                      | exit 0              |
| Tests     | `cargo test -p splitter`                                       | all pass            |
| Full test | `cargo test --workspace`                                       | all pass            |
| Lint      | `cargo clippy --workspace --all-targets -- -D warnings`        | exit 0, no warnings |
| Format    | `cargo fmt --all -- --check`                                   | exit 0              |

(`splitter` is the Tauri crate's package name — confirm with
`cargo metadata --no-deps --format-version 1 | grep -o '"name":"splitter[^"]*"'`
if unsure; `src-tauri/Cargo.toml` holds the exact `name`.)

## Suggested executor toolkit

- Invoke `superpowers:test-driven-development` mindset in reverse: these are
  characterization tests — write the assertion for what the code *currently*
  does, run it green immediately; if it is red, you have misread the behavior —
  re-read the cited lines, do **not** change production code to make it pass.

## Scope

**In scope** (the only files you should modify):

- `src-tauri/src/acceptor.rs` — add a `#[cfg(test)] mod tests` block.
- `src-tauri/src/commands/ops.rs` — add tests for `mute_all_core` /
  `disconnect_all_core`.
- `src-tauri/src/commands/peers.rs` — add tests for `teardown_session`.
- `src-tauri/src/commands/streams.rs` — add tests for `open_stream_core` error
  paths and `notify_remote` no-op safety.
- `src-tauri/src/core.rs` — only if you must add a helper accessor; prefer not
  to touch it.

**Minimal extraction allowed** (only where a `#[tauri::command]` cannot be
tested otherwise): extract the pure body of a handler into a `pub(crate) async fn
<name>_core(core: &AppCore, ..) -> ..` that the command wraps, mirroring
`disconnect_all_core`. Keep the wrapper a one-line delegate so behavior is
identical. Candidate: `open_session` (`streams.rs:49-77`) → `open_session_core`,
whose no-connection path leaves an orphan `Active` session (a real quirk worth
locking). Do this only if you write a test that would otherwise be impossible.

**Out of scope** (do NOT touch):

- Any production logic beyond the extraction above — no behavior changes.
- `crates/splitter-cli/**` and `crates/splitter-core/**` — covered by plans 019
  and 020. Do not add tests there here.
- The frontend (`src/`, TypeScript).

## Git workflow

- Branch: `advisor/018-characterization-command-acceptor`.
- Commit per logical group (e.g. `test(acceptor): characterize session request
  and stale eviction`), conventional-commit **title only**.
- Do **not** push or open a PR unless the operator instructs it.

## Steps

### Step 1: Establish the acceptor test scaffold and lock `SessionRequest`

In `src-tauri/src/acceptor.rs`, add a `#[cfg(test)] mod tests`. Write a helper
that builds a core and a driven acceptor:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use splitter_core::net::signaling::SignalingMessage;
    use tempfile::tempdir;
    use tokio::sync::broadcast;

    async fn new_core() -> Arc<AppCore> {
        AppCore::init(tempdir().unwrap().path()).await.expect("init")
    }

    fn driven_acceptor(core: Arc<AppCore>, peer: Uuid)
        -> broadcast::Sender<PeerEvent> {
        let (tx, rx) = broadcast::channel(16);
        spawn_acceptor(core, peer, rx, "127.0.0.1:9".parse().unwrap());
        tx
    }
    // ...
}
```

Write `session_request_registers_active_session`: send
`PeerEvent::Message(SignalingMessage::SessionRequest { session_id:
<new uuid>, requested_by: <requester uuid> })`, then poll
`core.sessions.snapshot().await` until it is non-empty (bounded loop with
`tokio::task::yield_now()` / short `tokio::time::sleep`, max ~1s), and assert
exactly one session exists with `state == Active` and
`remote_peer_id == requester`.

Write `session_request_evicts_stale_session_for_same_requester`: first drive one
`SessionRequest` for requester R (session S1), wait until active; then drive a
second `SessionRequest` for the same R with a new session id S2; assert the
snapshot ends with exactly one session, id == S2, and S1 is gone.

**Verify**: `cargo test -p splitter acceptor::` → new tests pass.

### Step 2: Lock `StreamControl` (SetMuted state + Close removal)

Precondition helper: create an active session with one stream. The cleanest path
is to build the stream through the acceptor's own `StreamOpen` branch (Step 4),
but for `StreamControl` you can seed state directly via the `SessionManager`:
`open_outgoing` + `accept` gives you an active session, then `add_stream(&sid,
Stream::new_negotiating(StreamId(0), route, port))` + `activate_stream`. Build a
minimal `StreamRoute` with `StreamRoute::new(Endpoint{..}, Endpoint{..},
CodecParams{..}, 1.0)` (see `acceptor.rs:117-132` for field shapes).

- `stream_control_set_muted_marks_session_stream_muted`: drive
  `StreamControl { stream_id: 0, action: SetMuted { muted: true } }` for a
  `peer_id` that equals the session's `remote_peer_id`; assert the stream's muted
  flag in `core.sessions.snapshot()` is `true`. (This is the behavior the CLI
  lacks — locking it here proves plan 020 must preserve it.)
- `stream_control_close_removes_stream`: drive
  `StreamControl { stream_id: 0, action: Close }`; assert the session has zero
  streams afterward.

**Verify**: `cargo test -p splitter acceptor::stream_control` → pass.

### Step 3: Lock `SessionResponse{false}`, `DeviceListResponse`, `PeerRenamed`, `Disconnected`

- `session_response_false_closes_session`: seed an active session, drive
  `SessionResponse { session_id: <that id>, accepted: false }`, assert the
  session's state is `Closed` (or absent from active snapshot — assert whichever
  the current `close` produces; `snapshot()` still lists it with
  `state == Closed`).
- `device_list_response_caches_remote_devices`: drive
  `DeviceListResponse { devices: vec![DeviceDescriptor{ id, name, kind }] }` for
  `peer_id`; assert `core.remote_devices.read().await.get(&peer_id)` equals the
  sent vec.
- `peer_renamed_updates_discovered_peer`: pre-insert a `DiscoveredPeer` into
  `core.peers` (build one as in `core.rs:262-267`), drive `PeerRenamed { peer_id:
  <that id string>, peer_name: "New" }`, assert the map entry's `peer_name` is
  `"New"`.
- `disconnect_tears_down_sessions_and_skips_reconnect`: seed an active session
  for `peer_id`, drive `PeerEvent::Disconnected { reason: "test".into() }`,
  assert all sessions for that peer are closed and the process does not hang
  (the reconnect is skipped because the peer is not dialable in a bare core).

**Verify**: `cargo test -p splitter acceptor::` → all pass.

### Step 4: Lock `StreamOpen` state effect

- `stream_open_adds_active_stream_to_session`: seed an active incoming session
  for `peer_id` (via `SessionRequest` from Step 1, reusing that session id). Drive
  `StreamOpen { session_id, stream_id: 1, source: Endpoint{..}, sink:
  Endpoint{ peer_id: <local>, device_id: "default" }, codec: CodecParams{ name:
  Codec::Opus, bitrate: 64000, frame_ms: 20 }, udp_port: 0 }`. Poll until the
  session snapshot shows a stream with id 1; assert it exists. The
  `StreamOpenAck` reply is not observable (no connected peer) — do not assert on
  it. If `open_stream_as_sink` cannot bind an output device in the test/CI
  environment and the stream is never added, treat that as the STOP condition
  below (device-dependent) and mark this test `#[ignore]` with a one-line reason,
  reporting it.

**Verify**: `cargo test -p splitter acceptor::stream_open` → pass or documented
`#[ignore]`.

### Step 5: Command-layer characterization

In `ops.rs`:
- `mute_all_core_mutes_every_stream`: seed two active sessions each with a
  stream, call `mute_all_core(&core).await`, assert every stream reads muted.
- `disconnect_all_core_closes_all_sessions`: seed sessions, call it, assert none
  remain active.

In `peers.rs`:
- `teardown_session_closes_streams_and_session`: seed an active session with a
  stream, call `teardown_session(&core, sid).await`, assert `Ok(())` and the
  session is closed with no live streams.
- `teardown_session_unknown_session_is_ok`: call with a random `SessionId`,
  assert it returns `Ok(())` (current code closes a non-existent session without
  error — lock that).

In `streams.rs`:
- `open_stream_core_session_not_found_errors`: call `open_stream_core(&core,
  Uuid::new_v4(), .., sink_peer, .., 64000)` with no such session; assert the
  error string contains `"not found"`.
- `open_stream_core_session_not_bound_to_peer_errors`: seed a session bound to
  peer A, call with `sink_peer = B`; assert the error string contains `"not
  bound to peer"`.
- `notify_remote_no_session_is_noop`: call `notify_remote(&core, Uuid::new_v4(),
  0, StreamAction::Close).await` and assert it returns without panicking (the fn
  returns `()`; the test passing is the assertion).

**Verify**: `cargo test -p splitter commands::` → all new tests pass.

### Step 6: Full gate

**Verify**:
- `cargo test --workspace` → all pass (baseline plus new tests).
- `cargo clippy --workspace --all-targets -- -D warnings` → clean.
- `cargo fmt --all -- --check` → clean.

## Test plan

New tests, by file:

- `src-tauri/src/acceptor.rs`: `session_request_registers_active_session`,
  `session_request_evicts_stale_session_for_same_requester`,
  `stream_control_set_muted_marks_session_stream_muted`,
  `stream_control_close_removes_stream`,
  `session_response_false_closes_session`,
  `device_list_response_caches_remote_devices`,
  `peer_renamed_updates_discovered_peer`,
  `disconnect_tears_down_sessions_and_skips_reconnect`,
  `stream_open_adds_active_stream_to_session` (possibly `#[ignore]`).
- `src-tauri/src/commands/ops.rs`: `mute_all_core_mutes_every_stream`,
  `disconnect_all_core_closes_all_sessions`.
- `src-tauri/src/commands/peers.rs`:
  `teardown_session_closes_streams_and_session`,
  `teardown_session_unknown_session_is_ok`.
- `src-tauri/src/commands/streams.rs`:
  `open_stream_core_session_not_found_errors`,
  `open_stream_core_session_not_bound_to_peer_errors`,
  `notify_remote_no_session_is_noop`.

Structural pattern to model after: the existing `#[tokio::test]` in
`src-tauri/src/core.rs:206` (AppCore::init in a tempdir) and the manager tests in
`crates/splitter-core/src/net/manager.rs:200-320` (seeding sessions/streams).

Verification: `cargo test --workspace` → all pass, including the ~15 new tests.

## Done criteria

Machine-checkable. ALL must hold:

- [ ] `cargo build -p splitter` exits 0.
- [ ] `cargo test --workspace` exits 0; the new tests above exist and pass.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` exits 0.
- [ ] `cargo fmt --all -- --check` exits 0.
- [ ] `git diff 217a31d..HEAD -- src-tauri/src/acceptor.rs` shows only additions
      inside `#[cfg(test)]` (no production-behavior changes), OR — if a `_core`
      extraction was needed — the wrapper is a one-line delegate.
- [ ] No files outside the in-scope list are modified (`git status`).
- [ ] `plans/README.md` status row for 018 updated.

## STOP conditions

Stop and report back (do not improvise) if:

- Testing a command requires standing up a **real Tauri runtime** (an actual
  `tauri::App` / `AppHandle`, `tauri::State` construction, or IPC) rather than
  just an `AppCore` + broadcast channel. Report which command and scope it out —
  do NOT fabricate a Tauri app in the test.
- The code at the cited line ranges does not match the excerpts (drift).
- `open_stream_as_sink` in Step 4 cannot bind an output device in the test
  environment (device-dependent failure). Mark that single test `#[ignore]` with
  a one-line reason and report; do not block the rest of the plan.
- A characterization assertion is red on first run. This means the behavior
  differs from this plan's reading — re-read the cited production lines and fix
  the *test's expectation*. Never edit production code to make a characterization
  test green.
- Any `_core` extraction would require changing a public signature used outside
  `src-tauri/` — report the blast radius instead.

## Maintenance notes

- These tests are the safety net for **plan 020** (control-plane unification).
  When 020 moves this logic into `splitter-core`, these tests must stay green;
  if 020 legitimately changes a behavior, update the corresponding test in the
  same PR with a note explaining why the old behavior was a bug.
- A reviewer should scrutinize that no test secretly relaxed a production
  invariant (e.g. asserting `Ok` where the code actually errors) and that any
  `_core` extraction left the `#[tauri::command]` wrapper behavior-identical.
- Deferred out of scope: exercising the outbound `send_to_peer` replies
  (`StreamOpenAck`, `DeviceListResponse`, `StreamRequest` fan-out) — those need a
  connected peer and belong to the loopback integration tests, not here.
</content>
</invoke>
