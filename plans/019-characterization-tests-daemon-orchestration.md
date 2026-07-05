# Plan 019: Lock the CLI daemon orchestration with characterization tests

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**:
> `git diff --stat 217a31d..HEAD -- crates/splitter-cli/src/commands/daemon/`
> If any in-scope file changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Priority**: P1
- **Effort**: M
- **Risk**: LOW
- **Depends on**: none
- **Category**: tests
- **Planned at**: commit `217a31d`, 2026-07-05

## Why this matters

The CLI daemon re-implements the same inbound-signaling control plane as the
Tauri app, in `crates/splitter-cli/src/commands/daemon/`. Its ~290-line
`peer_event_loop.rs` dispatches every inbound message and does disconnect
teardown + reconnect spawning; `reconnect.rs` runs the backoff loop; `context.rs`
holds the shared `DaemonContext`. None of this has tests.

Plan 020 will delete these files and route the CLI through a unified core
control plane. That is only safe if we first lock what the daemon does today —
**including its divergences from the Tauri path** (e.g. it never mirrors
`SetMuted` into session state, and its `SessionRequest` dedup keys on an existing
*Active* session instead of evicting stale ones). This plan characterizes those
behaviors so plan 020's extraction is provably behavior-preserving (or makes any
intended change visible as a deliberately updated test).

## Current state

### The handlers are already broken into private async fns

`peer_event_loop.rs` dispatches (`crates/splitter-cli/src/commands/daemon/peer_event_loop.rs:20-55`)
into these `async fn`s that each take `&DaemonContext`:

- `handle_session_request(ctx, peer_id, session_id, requested_by)` (`:58-97`)
- `handle_stream_open(ctx, peer_id, conn_tx, default_output, msg)` (`:99-178`)
- `handle_stream_control(ctx, peer_id, stream_id, action)` (`:180-230`)
- `handle_session_response_close(ctx, peer_id, session_id)` (`:232-254`)
- `handle_peer_disconnected(ctx, peer_id, reason)` (`:256-290`)

They are module-private, so tests must live **inside `peer_event_loop.rs`** in a
`#[cfg(test)] mod tests { use super::*; .. }` block (same module = can call
private fns). The top dispatch is `_ => {}` for all other messages
(`peer_event_loop.rs:42`) — `DeviceListRequest`/`Response`, `PeerRenamed`,
`StreamRequest` are silently ignored (this is the drift plan 020 addresses; here
we only lock what runs).

### `DaemonContext` is crate-visible and cheaply constructible in tests

`context.rs:15-24`:

```rust
#[derive(Clone)]
pub(crate) struct DaemonContext {
    pub identity: PeerIdentity,
    pub trust: Arc<RwLock<TrustStore>>,
    pub sessions: Arc<SessionManager>,
    pub stream_registry: Arc<StreamRegistry>,
    pub discovered: DiscoveredPeers,          // Arc<RwLock<HashMap<String, DiscoveredPeer>>>
    pub outgoing_connections: PeerConnections, // Arc<RwLock<HashMap<Uuid, PeerConnectionHandle>>>
    pub local_peer_id: Uuid,
}
```

All fields are `pub` within the crate, so a test in the same crate can build one:

```rust
fn test_ctx() -> DaemonContext {
    let dir = tempfile::tempdir().unwrap();
    let identity = PeerIdentity { peer_id: Uuid::new_v4(), peer_name: "test".into() };
    let local = identity.peer_id;
    DaemonContext {
        identity,
        trust: Arc::new(RwLock::new(
            TrustStore::load_or_create(&dir.path().join("trust.toml")).unwrap())),
        sessions: SessionManager::new(),
        stream_registry: StreamRegistry::new(),
        discovered: Arc::default(),
        outgoing_connections: Arc::default(),
        local_peer_id: local,
    }
}
```

(`SessionManager::new()` and `StreamRegistry::new()` both return `Arc<..>` —
`manager.rs:39`, `stream_runtime.rs:221`. `tempfile` is already a dev-dep of the
CLI crate — check `crates/splitter-cli/Cargo.toml`; if absent, add it under
`[dev-dependencies]` in that step and report.)

### Behavior each handler exhibits (what to lock)

- **`handle_session_request`** (`:58-97`): parses both uuids (bad uuid → early
  return, no state change). Looks for an existing session with
  `remote_peer_id == requester && state == Active`. **If found, it prints
  "re-opened existing session" and returns without touching state** (this is the
  CLI's dedup — different from Tauri's stale-eviction). Otherwise
  `register_incoming` + `accept`, leaving one Active session.
- **`handle_stream_control`** (`:180-230`): prints a line per action; collects
  session ids with `remote_peer_id == peer_id`. `Close` → `registry.close` per
  session. Non-close → `StreamControlSignal::from(action)` then
  `registry.send_control` per session. **It never calls `set_stream_muted`** — so
  after a `SetMuted{true}`, the session's stream muted flag stays `false`. Lock
  that (it is the divergence plan 020 must reconcile).
- **`handle_session_response_close`** (`:232-254`): closes the named session's
  streams then the session; prints.
- **`handle_peer_disconnected`** (`:256-290`): prints; tears down all
  sessions+streams for `peer_id`; then **only if there was an active session AND
  the peer is still present in `ctx.discovered`** calls
  `spawn_reconnect_loop(ctx.clone(), peer_id)`. In a bare test the discovered map
  is empty, so no reconnect task is spawned — teardown is fully observable and
  the test cannot hang.
- **`handle_stream_open`** (`:99-178`): builds a `StreamRoute`, resolves the
  output device (`"default"` → `default_output`), calls `open_stream_as_sink`
  (binds a real ephemeral UDP socket), sends `StreamOpenAck` down the passed
  `conn_tx` (an `mpsc::Sender<SignalingMessage>` — the test can create one with
  `tokio::sync::mpsc::channel(8)` and assert on the received ack). On success it
  prints; on error it sends `accepted:false`. **Note:** unlike the Tauri
  acceptor, the CLI's `handle_stream_open` does **not** call `add_stream` /
  `activate_stream` — it opens the runtime and acks but never records the stream
  in the `SessionManager`. Lock this exactly as-is (do not "fix" it).

### `context.rs::peer_display_name` (`:27-41`)

Resolution order: `discovered` map by `peer_id.to_string()` → `trust` store via
`peer_for` → `short(peer_id)` (first 8 chars). Pure enough to characterize with a
`DaemonContext`.

### `reconnect.rs` and the reconnect gate

`spawn_reconnect_loop` (`reconnect.rs:7-81`) is a spawned task that sleeps on the
backoff array `[1, 2, 4, 8, 16, 30, 30, 30, 30, 30]` (`reconnect.rs:9`) and calls
the real `connect_to_peer` — it is **not** directly unit-testable without a
network peer and multi-second sleeps. The characterizable part is the **gating
predicate**: "is the peer still present in `discovered`, and what address does it
resolve to". Today that logic is inlined (`reconnect.rs:14-23` and `:63-68`) and
duplicated in `peer_event_loop.rs:280-286`. See Step 4 for the allowed minimal
seam.

### Conventions to honor

- No code comments except a non-obvious WHY (`CLAUDE.md`). The CLI uses
  `#[allow(clippy::print_stdout)]` blocks around its `println!`s — keep tests
  from tripping that lint (tests don't print).
- Async tests: `#[tokio::test]`. Existing daemon tests are in
  `crates/splitter-cli/src/commands/daemon/mod.rs:262-342` (see
  `open_dedupe_returns_existing_session` and `graceful_shutdown_on_empty_state`
  for the seeding style) and `repl.rs`.
- Conventional-commit **title only**. **Never** add a `Co-Authored-By` trailer.

## Commands you will need

| Purpose   | Command                                                        | Expected on success |
|-----------|---------------------------------------------------------------|---------------------|
| Build     | `cargo build -p splitter-cli`                                 | exit 0              |
| Tests     | `cargo test -p splitter-cli`                                  | all pass            |
| Full test | `cargo test --workspace`                                      | all pass            |
| Lint      | `cargo clippy --workspace --all-targets -- -D warnings`       | exit 0, no warnings |
| Format    | `cargo fmt --all -- --check`                                  | exit 0              |

## Scope

**In scope** (the only files you should modify):

- `crates/splitter-cli/src/commands/daemon/peer_event_loop.rs` — add a
  `#[cfg(test)] mod tests` block.
- `crates/splitter-cli/src/commands/daemon/context.rs` — add tests for
  `peer_display_name`; optionally add the minimal reconnect-gate seam (Step 4).
- `crates/splitter-cli/src/commands/daemon/reconnect.rs` — tests only for an
  extracted gate helper if Step 4 is taken.
- `crates/splitter-cli/Cargo.toml` — only to add `tempfile` under
  `[dev-dependencies]` if it is not already there.

**Minimal seam allowed**: in Step 4 you may extract the "resolve a dialable
address for a still-present peer" predicate from `reconnect.rs` /
`peer_event_loop.rs` into a small pure `fn` (input: a `&HashMap<String,
DiscoveredPeer>` snapshot + `Uuid`, output: `Option<SocketAddr>`), then have both
existing sites call it. This is a behavior-preserving refactor that makes the
gate testable. Do it **only** if you write a test that needs it and it does not
require touching the spawned loop's structure.

**Out of scope** (do NOT touch):

- The `spawn_reconnect_loop` spawned-task body, `run(..)`, `graceful_shutdown`,
  `repl.rs`, `ui.rs` — no behavior changes.
- `src-tauri/**` and `crates/splitter-core/**` — plans 018 and 020.
- Any attempt to make `connect_to_peer` injectable across the daemon (that is
  plan 020's job; here it is a STOP condition).

## Git workflow

- Branch: `advisor/019-characterization-daemon-orchestration`.
- Commit per logical group (e.g. `test(daemon): characterize peer disconnect
  teardown`), conventional-commit **title only**.
- Do **not** push or open a PR unless the operator instructs it.

## Steps

### Step 1: Scaffold and lock `handle_session_request`

Add `#[cfg(test)] mod tests` to `peer_event_loop.rs` with the `test_ctx()` helper
from "Current state". Add a small seed helper that creates an active session:

```rust
async fn seed_active_session(ctx: &DaemonContext, remote: Uuid) -> SessionId {
    let sid = ctx.sessions.open_outgoing(ctx.local_peer_id, remote).await;
    ctx.sessions.accept(&sid).await.unwrap();
    sid
}
```

Tests:
- `session_request_registers_new_active_session`: fresh ctx, call
  `handle_session_request(&ctx, peer, &new_uuid.to_string(),
  &requester.to_string()).await`; assert one Active session with
  `remote_peer_id == requester`.
- `session_request_with_existing_active_session_is_noop`: seed an active session
  for `requester`, then call `handle_session_request` with a **new** session id
  for the same requester; assert the snapshot still has exactly one session and
  its id is the original (the new id was NOT registered — locks the dedup).
- `session_request_bad_uuid_changes_nothing`: call with
  `session_id = "not-a-uuid"`; assert zero sessions.

**Verify**: `cargo test -p splitter-cli handle_session_request` → pass.

### Step 2: Lock `handle_stream_control` and the SetMuted divergence

Seed an active session with a stream (use `SessionManager` directly:
`add_stream(Stream::new_negotiating(StreamId(0), route, port))` + `activate_stream`;
build `route` via `StreamRoute::new(Endpoint{..}, Endpoint{..}, CodecParams{..},
1.0)`). Use the seeded session's `remote_peer_id` as the `peer_id` argument.

- `stream_control_set_muted_does_not_touch_session_state`: call
  `handle_stream_control(&ctx, remote, 0, StreamAction::SetMuted { muted: true })`;
  assert the stream's muted flag in `ctx.sessions.snapshot()` is still `false`
  (the CLI never mirrors mute into session state — this is the drift to lock).
- `stream_control_close_closes_registry_entry`: register a real runtime for the
  stream via `open_stream_as_sink` first (or assert no-panic on an empty
  registry). Call `handle_stream_control(&ctx, remote, 0, StreamAction::Close)`
  and assert it returns without panicking. (The CLI does not remove the stream
  from the SessionManager here — do not assert removal.)

If seeding a real runtime is environment-dependent (device binding), assert the
no-panic path with an empty registry instead and note it.

**Verify**: `cargo test -p splitter-cli handle_stream_control` → pass.

### Step 3: Lock `handle_session_response_close` and `handle_peer_disconnected`

- `session_response_close_closes_session`: seed an active session, call
  `handle_session_response_close(&ctx, remote, &sid.to_string())`; assert the
  session is `Closed`.
- `peer_disconnected_tears_down_all_sessions_for_peer`: seed two active sessions
  for the same `remote` (and one for a different peer), call
  `handle_peer_disconnected(&ctx, remote, "test")`; assert both of `remote`'s
  sessions are closed and the unrelated peer's session is untouched.
- `peer_disconnected_without_discovery_entry_does_not_reconnect`: with an empty
  `ctx.discovered`, seed an active session and call `handle_peer_disconnected`;
  assert it returns promptly (no hang) — the reconnect loop is gated off. (You
  cannot directly assert "no task spawned"; the assertion is that the call
  completes and teardown happened.)

**Verify**: `cargo test -p splitter-cli handle_peer_disconnected` → pass.

### Step 4 (optional seam): Lock the reconnect gate predicate

Only if you want direct coverage of the "still present → dialable address" logic:
extract from `reconnect.rs:14-23` / `peer_event_loop.rs:280-286` a pure helper,
e.g. in `context.rs`:

```rust
pub(crate) fn dialable_addr(
    discovered: &HashMap<String, DiscoveredPeer>,
    peer_id: Uuid,
) -> Option<SocketAddr> {
    discovered
        .values()
        .find(|p| p.peer_id == peer_id.to_string())
        .and_then(|p| format!("{}:{}", p.host, p.port).parse().ok())
}
```

Then replace the two inlined lookups with a call to it (behavior-preserving), and
test:
- `dialable_addr_returns_none_for_absent_peer`.
- `dialable_addr_resolves_host_port_for_present_peer`.
- `dialable_addr_none_on_unparseable_host`.

**Verify**: `cargo test -p splitter-cli dialable_addr` → pass, and the existing
daemon behavior tests in `mod.rs` still pass.

### Step 5: Lock `peer_display_name`

In `context.rs` tests:
- `peer_display_name_prefers_discovered`: insert a `DiscoveredPeer` with a known
  name; assert it is returned.
- `peer_display_name_falls_back_to_short_uuid`: empty discovered + empty trust;
  assert the return is the 8-char prefix (`short(&peer_id)`).

**Verify**: `cargo test -p splitter-cli peer_display_name` → pass.

### Step 6: Full gate

**Verify**:
- `cargo test --workspace` → all pass.
- `cargo clippy --workspace --all-targets -- -D warnings` → clean.
- `cargo fmt --all -- --check` → clean.

## Test plan

New tests, by file:

- `peer_event_loop.rs`: `session_request_registers_new_active_session`,
  `session_request_with_existing_active_session_is_noop`,
  `session_request_bad_uuid_changes_nothing`,
  `stream_control_set_muted_does_not_touch_session_state`,
  `stream_control_close_closes_registry_entry`,
  `session_response_close_closes_session`,
  `peer_disconnected_tears_down_all_sessions_for_peer`,
  `peer_disconnected_without_discovery_entry_does_not_reconnect`.
- `context.rs`: `peer_display_name_prefers_discovered`,
  `peer_display_name_falls_back_to_short_uuid`, and (if Step 4 taken)
  `dialable_addr_*`.

Structural pattern to model after: `crates/splitter-cli/src/commands/daemon/mod.rs:314-329`
(`open_dedupe_returns_existing_session`) for session seeding, and
`crates/splitter-core/src/net/manager.rs:200-320` for stream seeding.

Verification: `cargo test --workspace` → all pass, including the ~10-13 new tests.

## Done criteria

Machine-checkable. ALL must hold:

- [ ] `cargo build -p splitter-cli` exits 0.
- [ ] `cargo test --workspace` exits 0; the new tests above exist and pass.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` exits 0.
- [ ] `cargo fmt --all -- --check` exits 0.
- [ ] Production changes, if any, are limited to the Step 4 seam extraction and
      are behavior-preserving (both original call sites now delegate to the
      helper); everything else is inside `#[cfg(test)]`.
- [ ] No files outside the in-scope list are modified (`git status`).
- [ ] `plans/README.md` status row for 019 updated.

## STOP conditions

Stop and report back (do not improvise) if:

- Injecting a fake for `connect_to_peer` (to test the reconnect *loop* rather
  than just its gate predicate) would require changing `spawn_reconnect_loop`'s
  signature or threading a connector trait through `DaemonContext` / `run()` /
  `register_outgoing_connection`. Report the blast radius (which signatures,
  which call sites) and stop — that unification belongs to plan 020.
- The cited line ranges do not match the live code (drift).
- A characterization assertion is red on first run — re-read the cited
  production lines and fix the *test's* expectation, never the production code.
- Seeding a stream runtime via `open_stream_as_sink` fails due to device/UDP
  constraints in CI — fall back to the empty-registry no-panic assertion and note
  it; do not block the plan.

## Maintenance notes

- These tests, together with plan 018's, are the safety net for **plan 020**.
  Several of them deliberately lock *divergent* CLI behavior (no SetMuted mirror,
  dedup-instead-of-evict, stream not recorded in SessionManager on open). Plan
  020 will have to decide, per divergence, whether to converge on the Tauri
  behavior — when it does, the corresponding test here must be updated in the
  same PR with a one-line rationale, not silently deleted.
- A reviewer should confirm no test asserts a *desired* behavior that the code
  does not currently exhibit (that would defeat the characterization purpose).
- Deferred out of scope: exercising the multi-second reconnect backoff loop and
  the outbound `StreamOpenAck` fan-out end-to-end — those belong to the
  integration/loopback tests.
</content>
