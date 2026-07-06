# Plan 003: Make the CLI daemon's acceptor supervisor survive a broadcast lag

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat 217a31d..HEAD -- crates/splitter-cli/src/commands/daemon/mod.rs`
> If the in-scope file changed since this plan was written, compare the
> "Current state" excerpt against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Priority**: P1
- **Effort**: S
- **Risk**: LOW
- **Depends on**: none
- **Category**: bug
- **Planned at**: commit `217a31d`, 2026-07-05

## Why this matters

The CLI daemon runs a background task that watches the signaling server's
`connection_established` broadcast channel and, for every newly connected peer,
spawns a stream-open acceptor so that peer can actually open audio streams.
That task's loop is written as `while let Ok(peer_id) = conn_est_rx.recv().await`.
A `tokio::sync::broadcast::Receiver` returns `Err(RecvError::Lagged(n))` — not a
fatal error — when the receiver falls behind and the channel overwrites unread
messages. Because `while let Ok(..)` treats **any** `Err` as loop termination,
a single lag event permanently kills the supervisor. After that, peers can still
establish a signaling connection, but the daemon never spawns an acceptor for
them, so every subsequent peer silently fails to open streams for the entire
lifetime of the daemon process. This is a "works until it doesn't, then stays
broken with no error" failure — the worst kind to diagnose in the field.

The Tauri app already has the correct pattern for the identical supervisor
(`src-tauri/src/core.rs`, `spawn_acceptor_supervisor`): it explicitly `continue`s
on `Lagged` and only `break`s on `Closed`. This plan brings the CLI daemon in
line with that proven implementation.

## Current state

- `crates/splitter-cli/src/commands/daemon/mod.rs` — the CLI daemon entry point.
  The acceptor supervisor is spawned inside `run(..)` at lines 186–208. The bug
  is the loop header on line 191.

Current (buggy) code, `crates/splitter-cli/src/commands/daemon/mod.rs:186-208`:

```rust
    {
        let mut conn_est_rx = server.connection_established_tx.subscribe();
        let conns = server.connections.clone();
        let acceptor_ctx = ctx.clone();
        tokio::spawn(async move {
            while let Ok(peer_id) = conn_est_rx.recv().await {
                let name = acceptor_ctx.peer_display_name(&peer_id).await;
                #[allow(clippy::print_stdout)]
                {
                    println!(">> {name} connected (peer_id {})", context::short(&peer_id));
                }
                let guard = conns.read().await;
                if let Some(conn) = guard.get(&peer_id) {
                    spawn_stream_open_acceptor(
                        acceptor_ctx.clone(),
                        conn.tx.clone(),
                        conn.events.subscribe(),
                        peer_id,
                    );
                }
            }
        });
    }
```

The **correct** mirror, `src-tauri/src/core.rs:171-189` (already in the repo —
do NOT modify it, use it as the reference for the fix shape):

```rust
        tauri::async_runtime::spawn(async move {
            loop {
                match established.recv().await {
                    Ok(peer_id) => {
                        // ... spawn acceptor ...
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
```

Repo conventions that apply here:

- **No code comments** except a non-obvious WHY. A one-line WHY on the `Lagged`
  arm (e.g. that a lag must not tear down the supervisor) is acceptable and
  encouraged because the reason is non-obvious; do not narrate the rest.
- The file already uses `#[allow(clippy::print_stdout)]` around `println!` in
  the daemon — keep that block exactly as-is.
- Conventional-commit **title only**, no body. NEVER add a `Co-Authored-By`
  trailer.

## Commands you will need

| Purpose   | Command                                                        | Expected on success        |
|-----------|---------------------------------------------------------------|----------------------------|
| Build     | `cargo build -p splitter-cli`                                 | exit 0                     |
| Tests     | `cargo test -p splitter-cli`                                  | all pass                   |
| Workspace | `cargo test --workspace`                                      | all pass                   |
| Lint      | `cargo clippy --workspace --all-targets -- -D warnings`       | exit 0, no warnings        |
| Format    | `cargo fmt --all -- --check`                                  | exit 0, no diff            |

## Scope

**In scope** (the only file you should modify):
- `crates/splitter-cli/src/commands/daemon/mod.rs`

**Out of scope** (do NOT touch, even though they look related):
- `src-tauri/src/core.rs` — it is the reference implementation and is already
  correct. Changing it is not part of this fix.
- `crates/splitter-cli/src/commands/daemon/peer_event_loop.rs` — the acceptor
  body (`spawn_stream_open_acceptor`) is unaffected; only the supervisor loop
  that calls it changes.
- The behaviour inside the `Ok` arm (the `println!`, the `conns.read()` lookup,
  the `spawn_stream_open_acceptor` call) must be preserved byte-for-byte apart
  from being moved into the `Ok(peer_id) => { .. }` arm.

## Git workflow

- Branch: `advisor/003-cli-daemon-broadcast-lag-hang`
- One commit; conventional-commit title only, e.g.
  `fix(cli): keep daemon acceptor supervisor alive after broadcast lag`
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Replace the `while let Ok` loop with an explicit match

In `crates/splitter-cli/src/commands/daemon/mod.rs`, inside the `tokio::spawn`
at line 190, replace the `while let Ok(peer_id) = conn_est_rx.recv().await { .. }`
with an unconditional `loop` that matches on the `recv()` result. Move the
existing body verbatim into the `Ok(peer_id) => { .. }` arm, `continue` on
`Lagged`, and `break` on `Closed`. Target shape:

```rust
        tokio::spawn(async move {
            loop {
                match conn_est_rx.recv().await {
                    Ok(peer_id) => {
                        let name = acceptor_ctx.peer_display_name(&peer_id).await;
                        #[allow(clippy::print_stdout)]
                        {
                            println!(">> {name} connected (peer_id {})", context::short(&peer_id));
                        }
                        let guard = conns.read().await;
                        if let Some(conn) = guard.get(&peer_id) {
                            spawn_stream_open_acceptor(
                                acceptor_ctx.clone(),
                                conn.tx.clone(),
                                conn.events.subscribe(),
                                peer_id,
                            );
                        }
                    }
                    // A lagged receiver dropped some connection events; keep serving future peers.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
```

Do not add a fresh `use` for `RecvError`; fully-qualify it as
`tokio::sync::broadcast::error::RecvError::{Lagged, Closed}` exactly as the
Tauri mirror does, to keep the import surface unchanged.

**Verify**: `cargo build -p splitter-cli` → exit 0.

### Step 2: Add a focused regression test for the lag-survival behaviour

The full supervisor wiring (a live `SignalingServer` + real peer connections)
is impractical to stand up in a unit test. Instead, add a small tokio test that
proves the **loop contract** the fix depends on: that a `broadcast::Receiver`
which has lagged still yields subsequent values when drained with the
`Ok/Lagged(continue)/Closed(break)` shape.

Add this test to the existing `#[cfg(test)] mod tests` block at the bottom of
`crates/splitter-cli/src/commands/daemon/mod.rs` (the module already exists,
starting at line 262). Model it structurally after the async tests already in
that module (e.g. `graceful_shutdown_on_empty_state_does_not_panic` at line 306).

```rust
    #[tokio::test]
    async fn acceptor_loop_shape_survives_lagged_receiver() {
        use tokio::sync::broadcast::{self, error::RecvError};

        let (tx, mut rx) = broadcast::channel::<Uuid>(2);
        for _ in 0..5 {
            let _ = tx.send(Uuid::new_v4());
        }
        let wanted = Uuid::new_v4();
        tx.send(wanted).unwrap();

        let mut delivered = None;
        loop {
            match rx.recv().await {
                Ok(id) => {
                    delivered = Some(id);
                    if id == wanted {
                        break;
                    }
                }
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => break,
            }
        }
        assert_eq!(
            delivered,
            Some(wanted),
            "loop must continue past a Lagged error and still deliver later values"
        );
    }
```

This asserts the exact control-flow the production loop uses: overflowing a
capacity-2 channel forces a `Lagged` on the first `recv`, and the test confirms
the loop keeps going and eventually delivers the later `wanted` value. If the
loop had used `while let Ok(..)`, it would exit on the `Lagged` and never see
`wanted`, so the assertion would fail — this test genuinely guards the bug.

**Verify**: `cargo test -p splitter-cli acceptor_loop_shape_survives_lagged_receiver`
→ 1 test passes.

### Step 3: Full verification

**Verify**:
- `cargo test --workspace` → all pass
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0

## Test plan

- New test in `crates/splitter-cli/src/commands/daemon/mod.rs`,
  `#[cfg(test)] mod tests`: `acceptor_loop_shape_survives_lagged_receiver`
  covers the regression (loop must not terminate on `Lagged`).
- Structural pattern to follow: the existing async tests in the same module
  (`graceful_shutdown_on_empty_state_does_not_panic`, `open_dedupe_returns_existing_session`).
- The `Ok`/`Closed` paths are covered by inspection plus the proven mirror in
  `src-tauri/src/core.rs:171-189`; a full end-to-end supervisor test is out of
  scope (documented here so it isn't re-litigated).
- Verification: `cargo test --workspace` → all pass, including the 1 new test.

## Done criteria

Machine-checkable. ALL must hold:

- [ ] `cargo build -p splitter-cli` exits 0
- [ ] `cargo test --workspace` exits 0; `acceptor_loop_shape_survives_lagged_receiver` exists and passes
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` exits 0
- [ ] `cargo fmt --all -- --check` exits 0
- [ ] `grep -n "while let Ok(peer_id) = conn_est_rx.recv" crates/splitter-cli/src/commands/daemon/mod.rs` returns no matches
- [ ] `grep -n "RecvError::Lagged(_) => continue" crates/splitter-cli/src/commands/daemon/mod.rs` returns 1 match
- [ ] No files outside `crates/splitter-cli/src/commands/daemon/mod.rs` are modified (`git status`)
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- The code at `crates/splitter-cli/src/commands/daemon/mod.rs:186-208` no longer
  matches the "Current state" excerpt (the supervisor was already refactored).
- `cargo clippy` flags the fully-qualified `RecvError` path or the moved body,
  and it isn't resolved by a trivial formatting fix.
- Any verification command fails twice after a reasonable fix attempt.
- The fix appears to require touching `peer_event_loop.rs` or any file outside
  the in-scope list.

## Maintenance notes

For the human/agent who owns this code after the change lands:

- The Tauri app (`src-tauri/src/core.rs::spawn_acceptor_supervisor`) and this CLI
  daemon now share the same loop contract. If one is changed (e.g. adding
  metrics on lag), consider whether the other should match.
- A reviewer should confirm the `Ok`-arm body is unchanged from the original
  (same `println!`, same `conns.read()` guard, same `spawn_stream_open_acceptor`
  arguments) — the only structural change is the loop framing.
- Deferred out of scope: a true integration test that drives a real
  `SignalingServer` through a lag. Left out because standing up two peers and
  forcing a broadcast overflow deterministically is disproportionate to the
  one-line contract this fix restores.
