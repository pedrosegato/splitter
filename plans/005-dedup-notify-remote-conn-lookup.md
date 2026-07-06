# Plan 005: Centralize the "find remote peer connection and notify it" pattern behind shared helpers

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat 217a31d..HEAD -- crates/splitter-core/src/net/signaling/client_ops.rs src-tauri/src/commands/peers.rs src-tauri/src/commands/streams.rs`
> If any in-scope file changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Priority**: P2
- **Effort**: S
- **Risk**: LOW
- **Depends on**: none
- **Category**: tech-debt
- **Planned at**: commit `217a31d`, 2026-07-05

## Why this matters

The pattern "given a session id, look up the remote peer, find its
`SignalingMessage` sender across the two connection maps (`server.connections`
and `outgoing`), then send it a control/notification message" is re-derived by
hand roughly eight times across the Tauri app and the CLI. Shared helpers for
the pieces already exist in
`crates/splitter-core/src/net/signaling/client_ops.rs`
(`find_conn`, `find_conn_tx`, `notify_remote_control`), but there is no helper
that does the whole *session → remote → send* sequence, so callers keep
re-inlining the two-map lookup. One concrete instance
(`src-tauri/src/commands/peers.rs::peer_devices`) even reimplements
`find_conn_tx`'s exact two-map fallback by hand instead of calling it. This is
low-severity but real drift risk: the two maps must always be consulted in the
same order, and every hand-rolled copy is a place that can silently diverge (or
already has).

This plan lands the missing shared helper `notify_remote_by_session` in
`client_ops.rs` and routes the two **low-risk** in-scope sites through existing
helpers: `peer_devices` onto `find_conn_tx`, and `streams.rs::notify_remote`
onto the new `notify_remote_by_session`. It deliberately leaves higher-risk
sites (notably `teardown_session`) untouched — see Scope.

## Current state

### Existing shared helpers (reuse these — do NOT reinvent)

`crates/splitter-core/src/net/signaling/client_ops.rs`:

- `find_conn(server_conns, outgoing_conns, peer_id) -> Option<ConnEndpoints>` — lines 21-47.
- `find_conn_tx(server_conns, outgoing_conns, peer_id) -> Option<mpsc::Sender<SignalingMessage>>` — lines 49-57. This is the canonical two-map tx lookup.
- `notify_remote_control(tx, stream_id, action)` — lines 159-167. Sends `SignalingMessage::StreamControl { stream_id, action }`.
- `pub type ConnectionMap = Arc<RwLock<HashMap<Uuid, PeerConnectionHandle>>>;` — line 13.

The file already imports `StreamAction` (line 3), `SignalingMessage` (line 3),
`Uuid` (line 11), `mpsc`/`RwLock` (line 10). It does **not** yet import
`SessionManager`. `tracing` is already a dependency of `splitter-core` (used in
`stream_runtime.rs`), so `tracing::warn!` can be called without a new import.

`SessionManager` is re-exported as `crate::net::manager::SessionManager` and its
`snapshot()` (`crates/splitter-core/src/net/manager.rs:145`) returns
`Vec<SessionSnapshot>`, where `SessionSnapshot { id: SessionId, remote_peer_id: Uuid, .. }`
(`session.rs:52,54`). `SessionId::get()` returns the inner `Uuid`.

### Site 1 — `peer_devices` reimplements `find_conn_tx` by hand

`src-tauri/src/commands/peers.rs:100-122`:

```rust
pub async fn peer_devices(
    core: State<'_, Arc<AppCore>>,
    peer_id: String,
) -> Result<Vec<DeviceDescriptor>, String> {
    let pid = uuid::Uuid::parse_str(&peer_id).map_err(|e| e.to_string())?;
    let cached = core.remote_devices.read().await.get(&pid).cloned();
    if cached.is_none() {
        let g = core.server.connections.read().await;
        let tx = g.get(&pid).map(|c| c.tx.clone());
        drop(g);
        let tx = if let Some(t) = tx {
            Some(t)
        } else {
            core.outgoing.read().await.get(&pid).map(|c| c.tx.clone())
        };
        if let Some(tx) = tx {
            tx.send(SignalingMessage::DeviceListRequest {}).await.ok();
        }
    }
    Ok(cached.unwrap_or_default())
}
```

The `server.connections`-then-`outgoing` fallback (the 8 lines building `tx`) is
exactly what `find_conn_tx` already does. `peers.rs` currently has **no**
`client_ops` import (imports are `peers.rs:1-7`).

### Site 2 — `streams.rs::notify_remote` re-derives session→remote→send

`src-tauri/src/commands/streams.rs:16-41`:

```rust
async fn find_peer_conn(core: &AppCore, peer_id: Uuid) -> Option<ConnEndpoints> {
    find_conn(&core.server.connections, &core.outgoing, peer_id).await
}

pub(crate) async fn notify_remote(core: &AppCore, sid: Uuid, stream_id: u8, action: StreamAction) {
    let snap = core.sessions.snapshot().await;
    let remote = match snap
        .iter()
        .find(|s| s.id.get() == sid)
        .map(|s| s.remote_peer_id)
    {
        Some(r) => r,
        None => {
            tracing::warn!(%sid, "notify_remote: session not found, skipping remote signal");
            return;
        }
    };
    match find_peer_conn(core, remote).await {
        Some(conn) => {
            notify_remote_control(&conn.tx, stream_id, action).await;
        }
        None => {
            tracing::warn!(%sid, %remote, "notify_remote: no live connection to remote peer, skipping remote signal");
        }
    }
}
```

`notify_remote` is called at `streams.rs:268` and `streams.rs:300`.
Import facts for `streams.rs` (lines 1-14):
- `notify_remote_control` is imported (line 3) but used **only** inside
  `notify_remote` (confirmed: `streams.rs:35` is its sole use). After Step 3 it
  must be removed from the import or clippy `-D warnings` will flag it unused.
- `find_peer_conn` is still used at `streams.rs:62, 108, 241` (stream-open
  flows), so it must **stay** — do not delete it even though `notify_remote`
  stops calling it.
- `find_conn` (line 3) remains used by `find_peer_conn` (line 17) — keep it.

Repo conventions that apply here:

- **No code comments** except a non-obvious WHY. None is needed here.
- Conventional-commit **title only**, no body. NEVER add a `Co-Authored-By` trailer.
- Result/error handling: app-layer functions return `Result<_, String>`; the two
  `tracing::warn!` diagnostics for "session not found" / "no live connection" are
  behaviour worth preserving — they move **into** the new core helper (Step 1) so
  no diagnostics are lost.

## Commands you will need

| Purpose        | Command                                                        | Expected on success        |
|----------------|---------------------------------------------------------------|----------------------------|
| Build core     | `cargo build -p splitter-core`                               | exit 0                     |
| Build app      | `cargo build -p splitter` (the tauri crate)                  | exit 0                     |
| Tests          | `cargo test --workspace`                                     | all pass                   |
| Lint           | `cargo clippy --workspace --all-targets -- -D warnings`      | exit 0, no warnings        |
| Format         | `cargo fmt --all -- --check`                                 | exit 0, no diff            |

(If `cargo build -p splitter` fails because the tauri crate name differs, run
`cargo build --workspace` instead — the workspace build covers all crates.)

## Scope

**In scope** (the only files you should modify):
- `crates/splitter-core/src/net/signaling/client_ops.rs` (add helper + test)
- `src-tauri/src/commands/peers.rs` (route `peer_devices` through `find_conn_tx`)
- `src-tauri/src/commands/streams.rs` (route `notify_remote` through the new helper; fix imports)

**Out of scope** (do NOT touch, even though they look related):
- `src-tauri/src/commands/peers.rs::teardown_session` (lines 177-219) — it also
  hand-rolls the two-map lookup (lines 196-208), **but a separate branch is
  landing a disconnect hard-kill fix here**. Do NOT refactor it; a merge
  conflict there would be costly. Leave it exactly as-is.
- `src-tauri/src/commands/peers.rs::broadcast_rename` (lines 135-148) — this is a
  fan-out to *all* connections, not a single-peer lookup; it is a different shape
  and not covered by `find_conn_tx`/`notify_remote_by_session`.
- CLI sites `crates/splitter-cli/src/commands/stream_repl.rs`
  (`stream_close`/`stream_volume`/`stream_set_paused`) and
  `crates/splitter-cli/src/commands/daemon/repl.rs::cmd_disconnect` — they match
  the same pattern and are candidates for the new helper, but they are **not** in
  the in-scope file list. Leave them for a follow-up (noted in Maintenance).
- `find_peer_conn` in `streams.rs` — keep it; it is still used by stream-open
  flows.

## Git workflow

- Branch: `advisor/005-dedup-notify-remote-conn-lookup`
- Commit per logical unit (helper+test, then the two call-site routings), or one
  commit for the set; conventional-commit title only, e.g.
  `refactor(net): add notify_remote_by_session and route app callers through it`
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Add `notify_remote_by_session` to `client_ops.rs`

In `crates/splitter-core/src/net/signaling/client_ops.rs`, add
`use crate::net::manager::SessionManager;` to the imports (top of file, near the
other `use crate::net::...` lines), then add this function after
`notify_remote_control` (after line 167):

```rust
pub async fn notify_remote_by_session(
    sessions: &SessionManager,
    server_conns: &ConnectionMap,
    outgoing_conns: &ConnectionMap,
    session_id: Uuid,
    stream_id: u8,
    action: StreamAction,
) {
    let snap = sessions.snapshot().await;
    let remote = match snap
        .iter()
        .find(|s| s.id.get() == session_id)
        .map(|s| s.remote_peer_id)
    {
        Some(r) => r,
        None => {
            tracing::warn!(%session_id, "notify_remote_by_session: session not found, skipping remote signal");
            return;
        }
    };
    match find_conn_tx(server_conns, outgoing_conns, remote).await {
        Some(tx) => notify_remote_control(&tx, stream_id, action).await,
        None => {
            tracing::warn!(%session_id, %remote, "notify_remote_by_session: no live connection to remote peer, skipping remote signal");
        }
    }
}
```

Then add a unit test in the existing `#[cfg(test)] mod tests` block (starts at
`client_ops.rs:169`), modeled on the existing `find_conn_tx_*` tests which use
the module's `empty_map()` (line 173) and `fake_handle()` (line 177) helpers:

```rust
    #[tokio::test]
    async fn notify_by_session_sends_control_to_remote() {
        let sessions = crate::net::manager::SessionManager::new();
        let local = Uuid::new_v4();
        let remote = Uuid::new_v4();
        let sid = sessions.open_outgoing(local, remote).await;
        sessions.accept(&sid).await.unwrap();

        let server = empty_map();
        let outgoing = empty_map();
        let (handle, mut rx) = fake_handle();
        server.write().await.insert(remote, handle);

        notify_remote_by_session(&sessions, &server, &outgoing, sid.get(), 7, StreamAction::Close)
            .await;

        match rx.recv().await {
            Some(SignalingMessage::StreamControl { stream_id, action }) => {
                assert_eq!(stream_id, 7);
                assert_eq!(action, StreamAction::Close);
            }
            other => panic!("expected StreamControl, got {other:?}"),
        }
    }
```

(`SessionManager::new()` returns `Arc<SessionManager>`; `&sessions` deref-coerces
to `&SessionManager` for the helper argument. `StreamAction` derives `PartialEq`,
so `assert_eq!` on it compiles.)

**Verify**: `cargo test -p splitter-core notify_by_session_sends_control_to_remote`
→ 1 test passes.

### Step 2: Route `peer_devices` through `find_conn_tx`

In `src-tauri/src/commands/peers.rs`:

1. Add the import (with the other `splitter_core::` imports at the top):
   `use splitter_core::net::signaling::client_ops::find_conn_tx;`
2. Replace the hand-rolled two-map lookup inside `peer_devices` (lines 108-120)
   with:

```rust
    if cached.is_none() {
        if let Some(tx) = find_conn_tx(&core.server.connections, &core.outgoing, pid).await {
            tx.send(SignalingMessage::DeviceListRequest {}).await.ok();
        }
    }
```

`SignalingMessage` is already imported in `peers.rs` (line 4). `find_conn_tx`
returns `Option<mpsc::Sender<SignalingMessage>>`, so `tx.send(SignalingMessage::DeviceListRequest {})`
type-checks unchanged.

**Verify**: `cargo build -p splitter` (or `cargo build --workspace`) → exit 0.

### Step 3: Route `streams.rs::notify_remote` through the new helper

In `src-tauri/src/commands/streams.rs`:

1. Update the `client_ops` import block (lines 2-5): **remove**
   `notify_remote_control` (it becomes unused) and **add**
   `notify_remote_by_session`. Keep `find_conn` (still used by `find_peer_conn`),
   `build_stream_route`, `stream_open_message`, `wait_for_stream_open_ack`,
   `ConnEndpoints`. Result:

```rust
use splitter_core::net::signaling::client_ops::{
    build_stream_route, find_conn, notify_remote_by_session, stream_open_message,
    wait_for_stream_open_ack, ConnEndpoints,
};
```

2. Replace the entire body of `notify_remote` (lines 20-41) with a delegation:

```rust
pub(crate) async fn notify_remote(core: &AppCore, sid: Uuid, stream_id: u8, action: StreamAction) {
    notify_remote_by_session(
        &core.sessions,
        &core.server.connections,
        &core.outgoing,
        sid,
        stream_id,
        action,
    )
    .await;
}
```

Do NOT touch `find_peer_conn` (lines 16-18) — it is still used at lines 62, 108,
241. The two `tracing::warn!` diagnostics are preserved because they now live in
`notify_remote_by_session` (Step 1).

**Verify**: `cargo build -p splitter` (or `cargo build --workspace`) → exit 0;
in particular no "unused import: `notify_remote_control`" warning.

### Step 4: Full verification

**Verify**:
- `cargo test --workspace` → all pass
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0

## Test plan

- New unit test in `crates/splitter-core/src/net/signaling/client_ops.rs`,
  `#[cfg(test)] mod tests`: `notify_by_session_sends_control_to_remote` — proves
  the helper resolves session → remote → server-map tx and delivers a
  `StreamControl` message. Model: the existing `find_conn_tx_*` tests in the same
  module (they already provide `empty_map()` / `fake_handle()`).
- The `find_conn_tx` and `notify_remote_control` primitives the helper composes
  already have their own tests in this module — no need to duplicate.
- `peer_devices` and `notify_remote` are thin routings onto tested helpers;
  their correctness is covered by the helper tests plus the workspace build.
- Verification: `cargo test --workspace` → all pass, including the 1 new test.

## Done criteria

Machine-checkable. ALL must hold:

- [ ] `cargo build --workspace` exits 0
- [ ] `cargo test --workspace` exits 0; `notify_by_session_sends_control_to_remote` exists and passes
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` exits 0
- [ ] `cargo fmt --all -- --check` exits 0
- [ ] `grep -n "notify_remote_by_session" crates/splitter-core/src/net/signaling/client_ops.rs` returns ≥2 matches (def + test)
- [ ] `grep -n "core.outgoing.read().await.get(&pid)" src-tauri/src/commands/peers.rs` returns no matches (the hand-rolled lookup in `peer_devices` is gone)
- [ ] `grep -n "notify_remote_control" src-tauri/src/commands/streams.rs` returns no matches (import removed; delegation used)
- [ ] `grep -n "find_peer_conn" src-tauri/src/commands/streams.rs` still returns its definition and the three stream-open call sites (it was NOT deleted)
- [ ] `teardown_session` in `peers.rs` is byte-for-byte unchanged
- [ ] No files outside the three in-scope files are modified (`git status`)
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- Any "Current state" excerpt no longer matches the live code (e.g.
  `teardown_session`'s disconnect-fix branch already merged and reshaped
  `peers.rs`, or `notify_remote` was already refactored).
- Removing `notify_remote_control` from the `streams.rs` import breaks a use you
  did not expect (i.e. it is used somewhere other than the old `notify_remote`
  body) — that contradicts a stated assumption; report it.
- `find_peer_conn` turns out to be unused after your edits (it should remain used
  at lines 62/108/241) — do not delete it to silence a warning; report the
  discrepancy.
- Any verification command fails twice after a reasonable fix attempt.
- The change appears to require touching `teardown_session`, `broadcast_rename`,
  or any out-of-scope file.

## Maintenance notes

For the human/agent who owns this code after the change lands:

- Follow-up (explicitly deferred): route the CLI sites
  (`stream_repl.rs::stream_close`/`stream_volume`/`stream_set_paused`,
  `daemon/repl.rs::cmd_disconnect`) and `peers.rs::teardown_session` through
  `notify_remote_by_session` once the disconnect hard-kill branch has merged.
  They were left out here to avoid a merge conflict and to keep this change
  low-risk.
- A reviewer should confirm: `find_peer_conn` is still present and used; the two
  `tracing::warn!` diagnostics are preserved (now in the core helper); no import
  went unused.
- The helper centralizes the two-map consultation order (server then outgoing).
  If a third connection map is ever added, `find_conn`/`find_conn_tx` are the
  single place to update, and `notify_remote_by_session` inherits it for free.
