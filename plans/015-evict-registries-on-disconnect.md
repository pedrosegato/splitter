# Plan 015: Evict connection registries on remote disconnect and pre-auth drop

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat 217a31d..HEAD -- src-tauri/src/acceptor.rs src-tauri/src/core.rs crates/splitter-core/src/net/signaling/server.rs`
> If any in-scope file changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Priority**: P1
- **Effort**: M
- **Risk**: MED
- **Depends on**: none (coordinates with plan 014 and the in-flight branch
  `fix/disconnect-hard-kill` — see "Coordination")
- **Category**: security / bug
- **Planned at**: commit `217a31d`, 2026-07-05

## Why this matters

Connection registries grow but never shrink. When a *remote* peer drops, the
acceptor's `Disconnected` arm tears down sessions/streams and spawns reconnect,
but it never removes the peer from `core.server.connections`, `core.outgoing`, or
`core.remote_devices`. The stale `PeerConnectionHandle` lives forever, and any
later `send_to_peer` finds that dead handle first and silently drops the message
into a closed mpsc channel — messages vanish with no error surfaced. Separately,
the signaling server's accept loop inserts *every* peer that sends a Hello —
including unauthenticated peers that are never accepted — into both `pending` and
`connections`, with no eviction if that connection closes before acceptance. A
hostile or buggy LAN device can open connections and send Hellos in a loop to grow
those maps without bound: an unauthenticated memory-exhaustion vector. The
codebase already has the correct pattern for the outgoing side
(`register_outgoing_connection` spawns a watcher that removes the entry on
`Disconnected`); this plan applies that same discipline to the remote-drop path
and to the pre-accept server path.

## Current state

The facts the executor needs, inlined:

- `src-tauri/src/core.rs` — `AppCore` owns the four registries (lines 41–53):
  - `pub server: SignalingServerHandle` — whose `connections:
    Arc<RwLock<HashMap<Uuid, PeerConnectionHandle>>>` is the inbound map.
  - `pub outgoing: Arc<RwLock<HashMap<Uuid, PeerConnectionHandle>>>` — outbound
    dials.
  - `pub remote_devices: Arc<RwLock<HashMap<Uuid, Vec<DeviceDescriptor>>>>` —
    cached device lists, keyed by peer.
  - `pub peers: Arc<RwLock<HashMap<String, DiscoveredPeer>>>` — mDNS discovery,
    keyed by peer_id *string*. This map is fed by discovery, NOT by connection
    lifecycle — do NOT evict it on disconnect (a peer can be discoverable while
    disconnected, and reconnect reads it). Leave `peers` alone.
  - core.rs has an existing `#[cfg(test)] mod tests` (lines 201–331) using
    `tempfile::tempdir()` and `AppCore::init` — model map-eviction unit tests
    after `apply_peer_rename_updates_existing_entry` (a pure-function test).

- `src-tauri/src/acceptor.rs` — the per-peer event loop `spawn_acceptor`
  (lines 24–433). The `Disconnected` arm (lines 391–424) currently:
  1. logs + emits `PeerDisconnected`,
  2. collects this peer's sessions and closes their streams + sessions (lines
     397–419),
  3. if there was an active session, calls `spawn_reconnect` (lines 420–422),
  4. `break`s.

  It never removes `peer_id` from `core.server.connections`, `core.outgoing`, or
  `core.remote_devices`. `send_to_peer` (lines 435–447) reads
  `core.server.connections` first, then `core.outgoing`, and on a stale handle
  sends into a closed channel and returns as if success.

- `crates/splitter-core/src/net/signaling/server.rs` — the accept loop. Inside
  the per-connection task (lines 106–209): after a valid Hello from a peer that is
  NOT auto-accepted, it pushes a `PendingPeer` (lines 200–207) AND inserts the
  handle into `connections` (line 208). There is no watcher that removes the
  pending/connections entry if that pre-accept connection drops before someone
  calls `accept_pending`. The auto-accept branch (lines 182–196) also inserts into
  `connections` (line 193) — that path *does* go on to be supervised by
  `spawn_acceptor` (via `connection_established_tx`), so its cleanup belongs to the
  acceptor path (this plan's core.rs eviction covers it); the *pending* branch has
  no such supervision until a human accepts.

- `crates/splitter-cli/src/commands/daemon/context.rs` — **the correct pattern to
  mirror.** `register_outgoing_connection` (lines 43–78): after inserting the
  handle into `outgoing_connections`, it subscribes to the handle's events and
  spawns a task that, on `PeerEvent::Disconnected` (or channel `Closed`), does
  `map.write().await.remove(&peer_id)` and breaks (lines 60–77). Replicate this
  removal discipline.

- `PeerConnectionHandle` (`crates/splitter-core/src/net/signaling/connection.rs`
  lines 24–28): `{ tx: mpsc::Sender<SignalingMessage>, events:
  broadcast::Sender<PeerEvent>, remote_addr: SocketAddr }`. Subscribe via
  `handle.events.subscribe()`.

- Repo conventions: no code comments except a non-obvious WHY (the race guard in
  Step 2 is exactly such a WHY — a one-line comment there is warranted). `NetError`
  for lib errors; the tauri side uses `tracing` for logs. Tests: `#[tokio::test]`
  with `tempdir()`.

## Coordination

- **Plan 014** edits `server.rs::accept_pending_as` (lines 238–278). This plan
  edits the *pre-accept Hello loop* (lines ~106–210). Different regions; if 014
  landed first, re-run the drift check and confirm the Hello loop still matches
  the excerpt before editing.
- **Branch `fix/disconnect-hard-kill`** is landing a fix for the **manual**
  disconnect path (user clicks disconnect) in `src-tauri/src/commands/peers.rs`,
  adding eviction + shutdown there. **This plan is strictly the REMOTE-DROP path
  (acceptor.rs) and the server pre-auth eviction (server.rs).** Do NOT edit
  `src-tauri/src/commands/peers.rs` and do NOT duplicate the manual-path logic.
  If that branch has already merged when you start, verify you are not
  re-implementing an eviction it already added on the remote path (it should not
  have — its scope is the manual command).

## Commands you will need

| Purpose      | Command                                                       | Expected on success |
|--------------|--------------------------------------------------------------|---------------------|
| Build        | `cargo build --workspace`                                    | exit 0              |
| Core tests   | `cargo test -p splitter-core signaling`                      | all pass            |
| Tauri tests  | `cargo test -p splitter` (or the src-tauri crate name)       | all pass            |
| Full tests   | `cargo test --workspace`                                     | all pass            |
| Lint         | `cargo clippy --workspace --all-targets -- -D warnings`      | exit 0, no warnings |
| Format       | `cargo fmt --all -- --check`                                 | exit 0              |

(If unsure of the src-tauri crate name, run `cargo metadata --no-deps --format-version 1 | grep -o '"name":"[^"]*"' | sort -u` or just use `cargo test --workspace`.)

## Scope

**In scope** (the only files you should modify):
- `src-tauri/src/acceptor.rs` — evict `peer_id` from the three connection maps in
  the `Disconnected` arm.
- `src-tauri/src/core.rs` — add a testable eviction helper on `AppCore` (and its
  unit test).
- `crates/splitter-core/src/net/signaling/server.rs` — spawn a pre-accept watcher
  that removes the `pending` + `connections` entry when a not-yet-accepted
  connection drops; add tests.

**Out of scope** (do NOT touch):
- `src-tauri/src/commands/peers.rs` — the manual-disconnect path owned by
  `fix/disconnect-hard-kill`.
- `core.peers` (mDNS map) — fed by discovery, not connection lifecycle.
- `crates/splitter-cli/src/commands/daemon/context.rs` — the reference
  implementation; read it, don't change it.
- The token-minting logic in `accept_pending_as` — that is plan 014.

## Git workflow

- Branch: `advisor/015-evict-registries-on-disconnect`
- Commit style: conventional-commit **title only**, e.g.
  `fix(net): evict connection registries on disconnect`. No body. **Never** add a
  `Co-Authored-By` trailer.
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Add an eviction helper on AppCore

In `src-tauri/src/core.rs`, add an async method that removes a peer from the three
connection-lifecycle maps (NOT `peers`):

```rust
impl AppCore {
    pub async fn evict_peer_connection(&self, peer_id: &Uuid) {
        self.server.connections.write().await.remove(peer_id);
        self.outgoing.write().await.remove(peer_id);
        self.remote_devices.write().await.remove(peer_id);
    }
}
```

Keeping this as one method makes the remote-drop path a single call and gives a
unit-testable surface.

**Verify**: `cargo build --workspace` → exit 0.

### Step 2: Call the helper from the acceptor's Disconnected arm, race-guarded

In `src-tauri/src/acceptor.rs`, in the `PeerEvent::Disconnected` arm (lines
391–424), after the session/stream teardown and BEFORE `break`, evict the peer
from the maps — but guard against racing a concurrent reconnect that may have
already re-inserted a *fresh* handle under the same `peer_id`.

The reconnect path (`src-tauri/src/reconnect.rs`) inserts the new handle into
`core.outgoing` keyed by the same `peer_id` and then spawns a new `spawn_acceptor`
for it. So a naive `remove(&peer_id)` here could delete the *reconnected* handle.
Guard by only removing the handle that belongs to *this* dead connection: compare
the stored handle's `remote_addr` (or a channel-closed check) against this loop's
`addr` before removing. Because this arm only fires when *this* connection's event
stream yields `Disconnected`, the safe rule is: remove the entry only if the
currently-stored handle's `tx` is closed (the live reconnected handle's `tx` is
open).

Target shape (inside the `Disconnected` arm, after the reconnect spawn, before
`break`):

```rust
// Only evict if the stored handle is the dead one — a concurrent reconnect
// may have already re-inserted a live handle under the same peer_id.
{
    let mut conns = core.server.connections.write().await;
    if conns.get(&peer_id).map(|h| h.tx.is_closed()).unwrap_or(false) {
        conns.remove(&peer_id);
    }
}
{
    let mut out = core.outgoing.write().await;
    if out.get(&peer_id).map(|h| h.tx.is_closed()).unwrap_or(false) {
        out.remove(&peer_id);
    }
}
core.remote_devices.write().await.remove(&peer_id);
```

Confirm `mpsc::Sender::is_closed` is available on the `tx` type
(`tokio::sync::mpsc::Sender` — it is). If it is not, fall back to comparing
`remote_addr` against `addr` (the loop's own address parameter) and only remove on
a match. `remote_devices` is a plain cache with no handle, so an unconditional
remove is fine there (a reconnect re-populates it via `DeviceListResponse`).

Note: order does not need to change relative to `spawn_reconnect`; evicting after
spawning reconnect is fine because reconnect's insert is gated on a successful
dial that takes at least 1s (see `reconnect.rs` backoff), so the `is_closed`
guard reliably distinguishes the dead handle from a future live one.

**Verify**: `cargo build --workspace` → exit 0.

### Step 3: Evict pre-accept connections in the server accept loop

In `crates/splitter-core/src/net/signaling/server.rs`, in the per-connection task
(lines 106–209), the branch that pushes a `PendingPeer` and inserts into
`connections` (lines 200–208) must also arrange for that entry to be removed if
the connection drops before it is accepted. Mirror
`context.rs::register_outgoing_connection` (lines 60–77).

After inserting into `connections` at line 208, subscribe to the same handle's
events and spawn a watcher that, on `PeerEvent::Disconnected` or channel `Closed`,
removes the peer from BOTH `pending` and `connections` — but only if it is still
pending (i.e. not yet accepted; once `accept_pending_as` has `take`n it from
`pending`, the acceptor path owns its lifecycle).

The subtlety: the handle is *moved* into `connections.insert(peer_uuid, handle)`,
so subscribe to `handle.events` BEFORE the insert. `PendingPeers` currently
exposes `list`, `take(idx)`, `push` (server.rs lines 31–47) but no
"remove by peer_id". Add a method:

```rust
impl PendingPeers {
    pub async fn remove_peer(&self, peer_id: &Uuid) -> bool {
        let mut guard = self.inner.lock().await;
        let before = guard.len();
        guard.retain(|p| &p.peer_id != peer_id);
        guard.len() != before
    }
}
```

Then, in the pending branch, before `c_inner.write().await.insert(peer_uuid, handle)`:

```rust
let mut pre_accept_events = handle.events.subscribe();
let pending_for_watch = p_inner.clone();
let conns_for_watch = c_inner.clone();
c_inner.write().await.insert(peer_uuid, handle);
tokio::spawn(async move {
    loop {
        match pre_accept_events.recv().await {
            Ok(PeerEvent::Disconnected { .. })
            | Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                // Evict only while still pending; once accepted, the acceptor owns cleanup.
                if pending_for_watch.remove_peer(&peer_uuid).await {
                    conns_for_watch.write().await.remove(&peer_uuid);
                }
                break;
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            Ok(_) => {}
        }
    }
});
```

The `if remove_peer(..)` guard is the accept-race protection: if
`accept_pending_as` already `take`d this peer out of `pending`, `remove_peer`
returns `false` and we do NOT touch `connections` (the accepted connection is now
owned by the supervisor/acceptor path). Borrow the `handle` clones carefully —
`handle.events.subscribe()` must be called before the value is moved into
`insert`.

**Verify**: `cargo build --workspace` → exit 0.

### Step 4: Unit-test the AppCore eviction helper

In `core.rs::tests`, add a `#[tokio::test]` that inserts a dummy entry into
`server.connections`, `outgoing`, and `remote_devices`, calls
`evict_peer_connection`, and asserts all three no longer contain the peer while an
unrelated peer in `peers` (mDNS) is untouched.

To get a `PeerConnectionHandle` for the test cheaply, connect a
`tokio::net::TcpStream` loopback pair and `spawn_peer_connection`, as the server
tests do — or, if constructing a handle is heavy, assert on `remote_devices`
(which holds plain `Vec<DeviceDescriptor>`, no handle) plus the fact that
`evict_peer_connection` removes from all three maps by inserting real handles from
a loopback pair. Model the loopback setup after
`server.rs::tests::server_queues_unknown_peer_hello` (lines 336–366).

**Verify**: `cargo test -p splitter --lib evict` (or `cargo test --workspace evict`)
→ the new test passes.

### Step 5: Test pre-accept eviction in the server

In `server.rs::tests`, add a `#[tokio::test]` `pre_accept_drop_evicts_pending_and_connections`:
1. Start a server (use the existing `setup()` helper, lines 295–334).
2. Connect a client via `TcpStream` + `spawn_peer_connection` and send a valid
   `Hello` with an unknown `peer_id` (so it lands in pending), as
   `server_queues_unknown_peer_hello` does (lines 336–366).
3. Poll until `server.pending.list().await` is non-empty AND
   `server.connections.read().await` contains the peer.
4. Drop the client handle (drop the `TcpStream`/`PeerConnectionHandle`) to close
   the connection.
5. Poll (with a bounded retry loop, ~50×50ms like the existing tests) until
   `server.pending.list().await.is_empty()` AND the peer is absent from
   `server.connections`.
6. Assert both maps are empty for that peer.

Also add `pending_peers_remove_peer_removes_matching` — a direct unit test of the
new `PendingPeers::remove_peer` (push two peers, remove one, assert the other
survives and the return bool is correct).

**Verify**: `cargo test -p splitter-core signaling` → all pass, including the 2
new tests.

### Step 6: Full gate

**Verify**:
- `cargo test --workspace` → all pass.
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0.
- `cargo fmt --all -- --check` → exit 0.

## Test plan

New tests:
- `core.rs`: `evict_peer_connection_removes_from_all_connection_maps` — the
  remote-drop eviction surface; asserts `peers` (mDNS) is untouched.
- `server.rs`: `pre_accept_drop_evicts_pending_and_connections` — the pre-auth
  flooding regression (dropped pre-accept connection is evicted).
- `server.rs`: `pending_peers_remove_peer_removes_matching` — unit test of the new
  `remove_peer`.
- Structural patterns: `core.rs::tests::apply_peer_rename_updates_existing_entry`
  (pure helper test) and `server.rs::tests::server_queues_unknown_peer_hello`
  (loopback Hello) and `accept_pending_promotes_and_acks`.
- Verification: `cargo test --workspace` → all pass, 3+ new tests.

## Done criteria

Machine-checkable. ALL must hold:

- [ ] `cargo build --workspace` exits 0
- [ ] `cargo test --workspace` exits 0; the new eviction tests exist and pass
- [ ] `grep -n "evict_peer_connection\|is_closed\|remote_devices.write" src-tauri/src/acceptor.rs`
      shows the Disconnected arm now evicts from the maps
- [ ] `grep -n "remove_peer" crates/splitter-core/src/net/signaling/server.rs`
      shows the new method and its use in the pre-accept watcher
- [ ] `git status` shows `src-tauri/src/commands/peers.rs` is UNMODIFIED
      (manual path is out of scope)
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` exits 0
- [ ] `cargo fmt --all -- --check` exits 0
- [ ] No files outside the in-scope list are modified (`git status`)
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- The code at the locations in "Current state" doesn't match the excerpts (e.g.
  the `Disconnected` arm already evicts, or `PendingPeers` already has a
  removal method, or `fix/disconnect-hard-kill` already added remote-path
  eviction).
- The reconnect race cannot be isolated in a test: if you cannot write a
  deterministic test proving the guard removes the *dead* handle but preserves a
  *reconnected* handle under the same `peer_id`, STOP and report with the race
  described (timing-dependent behavior in the eviction guard) rather than shipping
  an unverified guard. Ship the pre-accept server eviction (Step 3/5, which IS
  deterministically testable) and report the acceptor-race test as blocked.
- `mpsc::Sender::is_closed` is unavailable on the `tx` type AND `remote_addr`
  comparison is also insufficient to distinguish the dead handle from a
  reconnected one.
- Removing the pending/connections entry in Step 3 appears to require touching
  `accept_pending_as` (plan 014's region) — that means the ownership boundary
  differs from what this plan assumed.

## Maintenance notes

- The eviction guard depends on the invariant "a reconnected handle's `tx` is
  open while a dead handle's `tx` is closed." If reconnect ever inserts a handle
  before its channel is live, revisit the `is_closed` guard.
- `core.peers` (mDNS) is intentionally NOT evicted here — a peer stays
  discoverable while disconnected so reconnect can find its address. Don't "fix"
  that.
- The pre-accept watcher only cleans up peers still in `pending`. Once accepted,
  cleanup is the acceptor path's job (this plan's core.rs eviction). Keep those
  two ownership domains distinct.
- Reviewer should scrutinize: the accept-race (`remove_peer` returning `false`
  when the peer was already `take`n) and the reconnect-race (`is_closed` guard).
  These are the two concurrency hazards this plan trades on.
