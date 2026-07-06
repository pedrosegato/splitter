# Plan 020: Unify the CLI and Tauri signaling control plane in splitter-core

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**:
> `git diff --stat 217a31d..HEAD -- src-tauri/src/acceptor.rs src-tauri/src/reconnect.rs src-tauri/src/core.rs crates/splitter-cli/src/commands/daemon/ crates/splitter-core/src/net/`
> If any in-scope file changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition. In particular, if plan 003 has landed,
> the CLI supervisor loop will already be lag-tolerant — see "Interaction with
> plans 003/005/015" below.

## Status

- **Priority**: P2
- **Effort**: L
- **Risk**: HIGH
- **Depends on**: plans/018-characterization-tests-command-and-acceptor.md AND
  plans/019-characterization-tests-daemon-orchestration.md (both must be DONE and
  green before starting — they are this plan's safety net)
- **Category**: tech-debt
- **Planned at**: commit `217a31d`, 2026-07-05

## Why this matters

The CLI daemon and the Tauri app independently re-implement the **same** inbound
signaling control plane — the peer-event loop, the acceptor, disconnect
teardown, reconnect backoff, and the connection-established supervisor. Two
copies of security- and correctness-sensitive code have already **drifted into
divergent bugs**:

- The CLI's peer-event loop `_ => {}` (`peer_event_loop.rs:42`) silently ignores
  `DeviceListRequest`/`DeviceListResponse`, `PeerRenamed`, and `StreamRequest`
  that the Tauri acceptor fully handles (`acceptor.rs:320-388`).
- The CLI never mirrors `SetMuted` into session state; Tauri does
  (`acceptor.rs:281-288`).
- The two `SessionRequest` policies differ: Tauri **evicts** stale sessions from
  the same requester (`acceptor.rs:47-56`); the CLI **dedups** by returning early
  if an Active session already exists (`peer_event_loop.rs:71-85`).
- The reconnect backoff array `[1,2,4,8,16,30,30,30,30,30]` is copy-pasted in
  `src-tauri/src/reconnect.rs:17` and
  `crates/splitter-cli/src/commands/daemon/reconnect.rs:9`.
- The CLI supervisor has the broadcast-lag hang that plan 003 fixes; the Tauri
  supervisor already handles `Lagged` correctly (`core.rs:185`).

Every future signaling change has to be made twice and kept in sync by hand.
This plan hoists the shared logic into `splitter-core` behind a small observer
trait that captures the only real differences — CLI `println!` vs Tauri event
`emit`, and each frontend's auxiliary maps — then deletes both copies. The two
frontends become thin observer implementations. This plan **supersedes** the
point-fixes described for lag / stale-eviction / reconnect dedup and must fold
each of those fixes into the unified implementation rather than regress them.

## Current state

Read these files **end to end** before writing any code — this plan cites the
key seams but the executor must hold both implementations in mind:

- `src-tauri/src/acceptor.rs` (447 LOC) — Tauri inbound loop `spawn_acceptor`
  (`:24`), message handling (`:35-389`), disconnect teardown (`:391-424`),
  `send_to_peer` (`:435-447`).
- `crates/splitter-cli/src/commands/daemon/peer_event_loop.rs` (290 LOC) — CLI
  inbound loop `spawn_stream_open_acceptor` (`:12`), per-message handlers
  (`:58-254`), disconnect teardown + reconnect spawn (`:256-290`).
- `src-tauri/src/reconnect.rs` (`spawn_reconnect`, backoff `:17`) and
  `crates/splitter-cli/src/commands/daemon/reconnect.rs` (`spawn_reconnect_loop`,
  backoff `:9`) — near-identical reconnect loops; note the address-resolution
  differences (Tauri consults `core.peers` **and** `core.outgoing`; CLI consults
  only `discovered`).
- Supervisors: `src-tauri/src/core.rs::spawn_acceptor_supervisor` (`:168-191`,
  already lag-tolerant) and the CLI's inline supervisor in
  `crates/splitter-cli/src/commands/daemon/mod.rs:186-208` (`while let Ok(..)` —
  the plan-003 bug).
- `crates/splitter-cli/src/commands/daemon/context.rs` — `DaemonContext` and its
  `register_outgoing_connection` (`:43-78`) which re-spawns the acceptor on
  reconnect.
- `src-tauri/src/commands/streams.rs::open_stream_core` (`:79-183`) — the
  Tauri-only path invoked by the acceptor's `StreamRequest` branch
  (`acceptor.rs:356-388`); the CLI has no equivalent.

### The shared vs frontend-specific split

**Shared (identical logic, must move to core):** parsing/validation of message
fields; the session/stream state transitions via `SessionManager` +
`StreamRegistry` (`register_incoming`, `accept`, `add_stream`, `activate_stream`,
`set_stream_muted`, `remove_stream`, `close`, `snapshot`); building
`StreamRoute`; `open_stream_as_sink`; the disconnect teardown sequence; the
reconnect backoff loop and its gate; the connection-established supervisor's
lag-tolerant recv loop.

**Frontend-specific (must go through the observer):**
- **User feedback:** CLI prints `>> ...` lines (e.g.
  `peer_event_loop.rs:79-83, 95, 161-165, 191-206, 252, 260`); Tauri emits typed
  events `IncomingSession`, `SnapshotChanged`, `PeersChanged`, `PeerDisconnected`
  (`acceptor.rs:99-104, 224, 296, 318, 339, 350-353, 393-396`).
- **Auxiliary state Tauri keeps but CLI does not:** `remote_devices` map
  (`DeviceListResponse`, `acceptor.rs:338`), `peers` map rename
  (`PeerRenamed`, `acceptor.rs:341-355`), and the `StreamRequest` → open-as-source
  flow (`acceptor.rs:356-388`).
- **Sending replies to the peer:** Tauri's `send_to_peer` (looks up
  `core.server.connections` / `core.outgoing`) vs the CLI passing an explicit
  `conn_tx: mpsc::Sender<SignalingMessage>` into the handler.
- **Reconnect address resolution:** differs as noted above.
- **Two policy divergences to resolve (see STOP conditions):** SetMuted→session
  state, and SessionRequest evict-vs-dedup. Plans 018/019 lock both current
  behaviors; converging them is a deliberate change that must update the
  corresponding characterization test with a rationale.

### Conventions to honor

- New core code goes under `crates/splitter-core/src/net/signaling/` (siblings:
  `message.rs`, `client.rs`, `connection.rs`, `server.rs`, `client_ops.rs`).
  Follow the existing module layout (`net/signaling/mod.rs` re-exports).
- No code comments except a non-obvious WHY (`CLAUDE.md`). The daemon `mod.rs`
  already has a legitimate WHY block on teardown ordering (`mod.rs:213-218`) —
  preserve that rationale wherever the teardown moves.
- Errors are explicit `Result`; do not swallow. Logging via `tracing`.
- Conventional-commit **title only**. **Never** add a `Co-Authored-By` trailer.

## Commands you will need

| Purpose     | Command                                                       | Expected on success |
|-------------|--------------------------------------------------------------|---------------------|
| Build core  | `cargo build -p splitter-core`                               | exit 0              |
| Build all   | `cargo build --workspace`                                    | exit 0              |
| Tests (018) | `cargo test -p splitter`                                     | all pass (018 green)|
| Tests (019) | `cargo test -p splitter-cli`                                 | all pass (019 green)|
| Core tests  | `cargo test -p splitter-core`                                | all pass            |
| Full test   | `cargo test --workspace`                                     | all pass            |
| Lint        | `cargo clippy --workspace --all-targets -- -D warnings`      | exit 0, no warnings |
| Format      | `cargo fmt --all -- --check`                                 | exit 0              |

## Suggested executor toolkit

- `superpowers:systematic-debugging` if a characterization test from 018/019 goes
  red during extraction — a red test means the behavior changed; treat it as a
  signal, find the divergence, don't paper over it.
- Keep 018/019 running continuously: after **every** step run both
  `cargo test -p splitter` and `cargo test -p splitter-cli`. Their staying green
  is the definition of "behavior preserved".

## Scope

**In scope:**

- New module(s) under `crates/splitter-core/src/net/signaling/` — e.g.
  `control_plane.rs` (the shared event loop + teardown + supervisor helper) and
  `reconnect.rs` (the unified backoff loop + gate), plus the observer trait. Wire
  them into `crates/splitter-core/src/net/signaling/mod.rs`.
- `src-tauri/src/acceptor.rs` — reduce to a `ControlPlaneObserver` impl + a thin
  call into the core loop; delete the migrated body.
- `src-tauri/src/reconnect.rs` — delete in favor of the core reconnect loop (or
  reduce to a tiny adapter if the address-resolution seam requires it).
- `src-tauri/src/core.rs` — `spawn_acceptor_supervisor` delegates to the core
  supervisor helper.
- `crates/splitter-cli/src/commands/daemon/peer_event_loop.rs` — reduce to an
  observer impl + thin call; delete the migrated body.
- `crates/splitter-cli/src/commands/daemon/reconnect.rs` — delete in favor of the
  core reconnect loop.
- `crates/splitter-cli/src/commands/daemon/mod.rs` — supervisor now uses the core
  helper (folds in plan 003's lag fix); `context.rs::register_outgoing_connection`
  routes through the shared loop.

**Out of scope:**

- The data plane (`stream_runtime.rs` pumps, codec, jitter, FEC) — untouched.
- `SignalingServer` / `connection.rs` transport internals — untouched except
  possibly adding a small accessor if the supervisor helper needs one; if a new
  public API on the server is required, that is a STOP-and-confirm.
- The frontend TypeScript (`src/`).
- Changing the observable event/print wording — preserve each frontend's exact
  user-facing strings and emitted event types.

## Interaction with plans 003 / 005 / 015

This plan **absorbs** three point-fixes. Do not let the unification regress them:

- **Lag (plan 003):** the unified connection-established supervisor MUST
  `continue` on `broadcast::error::RecvError::Lagged` and only `break` on
  `Closed` (the Tauri pattern, `core.rs:185`). If plan 003 already landed in the
  CLI, the unified helper simply preserves it; if not, the unified helper fixes
  the CLI as a side effect. Either way, after this plan the CLI supervisor is
  lag-tolerant. If plan 003 is still TODO when you finish, mark it REJECTED in
  `plans/README.md` with the rationale "folded into 020".
- **Stale-session eviction ("015"):** the unified `SessionRequest` handler MUST
  evict stale sessions from the same requester (the Tauri behavior,
  `acceptor.rs:47-56`) — this is the safer policy. Converging the CLI onto it is
  an intended behavior change: update the plan-019 test
  `session_request_with_existing_active_session_is_noop` to the eviction
  expectation, in this PR, with a one-line rationale.
- **Reconnect dedup ("005"):** there is exactly **one** backoff array and one
  reconnect loop after this plan. Both frontends call it.

If plans 003/015/005 exist as separate plan files at execution time, note in each
of their README rows that 020 supersedes them.

## Steps

> After every step: `cargo test -p splitter && cargo test -p splitter-cli` must
> stay green. If a test goes red and the change was meant to be
> behavior-preserving, stop and diagnose before continuing.

### Step 1: Define the observer trait and the shared context abstraction in core

Create `crates/splitter-core/src/net/signaling/control_plane.rs`. Define:

- A `ControlPlaneObserver` trait with one method per frontend-specific effect,
  e.g. (adjust names to fit): `on_session_opened(&self, requester: Uuid,
  session: SessionId)`, `on_stream_opened(&self, peer: Uuid, stream: StreamId,
  source_device: &str, sink_device: &str)`, `on_stream_control(&self, peer: Uuid,
  stream: StreamId, action: &StreamAction)`, `on_session_closed(&self, peer:
  Uuid, session: SessionId)`, `on_peer_connected(&self, peer: Uuid)`,
  `on_peer_disconnected(&self, peer: Uuid, reason: &str)`,
  `on_devices_received(&self, peer: Uuid, devices: Vec<DeviceDescriptor>)`,
  `on_peer_renamed(&self, peer_id: &str, name: &str)`, and a hook for
  `StreamRequest` (`on_stream_requested(..)`) that Tauri implements (open as
  source) and the CLI can no-op or implement. Methods that a given frontend does
  not need get a default empty body.
- A shared handle carrying the common dependencies the loop needs:
  `sessions: Arc<SessionManager>`, `stream_registry: Arc<StreamRegistry>`, and a
  way to send a reply to the peer. Model the reply channel on the CLI's explicit
  `conn_tx: mpsc::Sender<SignalingMessage>` (the simplest, testable seam) — the
  Tauri side already resolves a tx and can pass it in the same shape (its
  `send_to_peer` becomes: resolve tx, hand it to the loop).

This step is additive — nothing calls it yet.

**Verify**: `cargo build -p splitter-core` → exit 0; `cargo test --workspace` →
still all green (no behavior touched yet).

### Step 2: Implement the shared message handling in core, with unit tests

Port the message-dispatch logic (SessionRequest with **eviction**, StreamOpen,
StreamControl with **SetMuted→session state**, SessionResponse close, DeviceList
request/response, PeerRenamed, StreamRequest, Disconnected teardown) into
`control_plane.rs`, calling observer methods at each frontend-specific point and
sending replies through the passed `conn_tx`. Add core unit tests (mirroring the
seeding style in `manager.rs:200-320`) with a `Vec`-recording test observer that
captures the callbacks, asserting: eviction on duplicate SessionRequest,
SetMuted mirrored into session state, StreamOpenAck sent on the reply channel,
teardown on Disconnected.

Do **not** wire any frontend to it yet.

**Verify**: `cargo test -p splitter-core control_plane` → new tests pass;
`cargo test --workspace` → still green.

### Step 3: Move the reconnect loop and supervisor helper into core

- Create the unified reconnect loop (single backoff array
  `[1,2,4,8,16,30,30,30,30,30]`, single gate). Because address resolution differs
  between frontends, take the resolver as a small closure/trait object
  (`Fn(Uuid) -> Option<SocketAddr>`) so Tauri can consult `peers`+`outgoing` and
  the CLI can consult `discovered`. On successful reconnect it re-enters the
  shared loop for the peer.
- Create a lag-tolerant `spawn_acceptor_supervisor` helper in core that both
  frontends call, subscribing to the server's `connection_established` broadcast
  and spawning the shared loop per peer. It MUST `continue` on `Lagged`, `break`
  on `Closed` (folds in plan 003).

**Verify**: `cargo test -p splitter-core` → green; `cargo test --workspace` →
still green (frontends not yet switched).

### Step 4: Switch the Tauri app onto the shared core

Reduce `src-tauri/src/acceptor.rs` to a `ControlPlaneObserver` impl whose
callbacks perform the Tauri effects (`core.emit(..)`, `remote_devices` insert,
`apply_peer_rename` + `PeersChanged`, `open_stream_core` for StreamRequest) and a
thin `spawn_acceptor` that resolves the peer tx (former `send_to_peer` logic) and
calls the core loop. Point `core.rs::spawn_acceptor_supervisor` at the core
supervisor helper. Replace `src-tauri/src/reconnect.rs` with the Tauri resolver
closure feeding the core reconnect loop (delete the duplicated body).

**Verify**: `cargo test -p splitter` → **all 018 characterization tests still
green** (this is the proof the Tauri behavior is preserved);
`cargo clippy -p splitter --all-targets -- -D warnings` → clean.

### Step 5: Switch the CLI daemon onto the shared core

Reduce `peer_event_loop.rs` to a `ControlPlaneObserver` impl whose callbacks
`println!` the existing `>> ...` lines (preserve exact wording) and a thin
spawner that hands the connection's `conn_tx` + a subscribed event receiver to
the core loop. Delete
`crates/splitter-cli/src/commands/daemon/reconnect.rs` and route reconnect
through the core loop with the CLI resolver (consults `discovered`). Update
`context.rs::register_outgoing_connection` and `mod.rs:186-208` to use the core
supervisor helper (this replaces the `while let Ok(..)` loop — folds in plan
003).

Because the CLI now handles the previously-ignored messages
(DeviceList*/PeerRenamed/StreamRequest) and converges on eviction + SetMuted
mirroring, update the plan-019 tests that locked the old CLI-only behavior
(`session_request_with_existing_active_session_is_noop`,
`stream_control_set_muted_does_not_touch_session_state`) to the new unified
expectations, each with a one-line rationale comment in the commit message (not
the code).

**Verify**: `cargo test -p splitter-cli` → green (updated 019 tests reflect the
convergence); `cargo clippy -p splitter-cli --all-targets -- -D warnings` →
clean.

### Step 6: Remove the dead duplication and full gate

Confirm the old bodies are gone:
- `git grep -n "1, 2, 4, 8, 16, 30, 30, 30, 30, 30"` → appears in **exactly one**
  file (the core reconnect module).
- `git grep -nc "SignalingMessage::StreamOpen {" src-tauri/src/acceptor.rs crates/splitter-cli/src/commands/daemon/peer_event_loop.rs`
  → the large duplicated match bodies are gone (only observer impls remain).

**Verify**:
- `cargo build --workspace` → exit 0.
- `cargo test --workspace` → all pass (018 + 019 + new core tests).
- `cargo clippy --workspace --all-targets -- -D warnings` → clean.
- `cargo fmt --all -- --check` → clean.

## Test plan

- **Regression safety net:** the full 018 (Tauri) and 019 (CLI) characterization
  suites must remain green through Steps 4-5, except the two 019 tests that lock
  now-converged CLI behavior, which are updated in Step 5.
- **New core unit tests** in `control_plane.rs` (Step 2) and the reconnect module
  (Step 3) using a recording test observer: eviction on duplicate SessionRequest,
  SetMuted mirrored into session state, StreamOpenAck emitted on the reply
  channel, Disconnected teardown, supervisor `continue`s on a simulated `Lagged`.
- Model core tests after `crates/splitter-core/src/net/manager.rs:200-320`
  (seeding) and `crates/splitter-core/tests/loopback_two_daemons.rs` (real
  two-server wiring) if an integration-level check is warranted.

Verification: `cargo test --workspace` → all pass.

## Done criteria

Machine-checkable. ALL must hold:

- [ ] `cargo build --workspace` exits 0.
- [ ] `cargo test --workspace` exits 0; 018 + 019 suites pass (with the two
      documented 019 updates), plus the new core tests.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` exits 0.
- [ ] `cargo fmt --all -- --check` exits 0.
- [ ] `git grep -n "1, 2, 4, 8, 16, 30, 30, 30, 30, 30"` returns exactly one
      match (the unified core reconnect module).
- [ ] `src-tauri/src/reconnect.rs` and
      `crates/splitter-cli/src/commands/daemon/reconnect.rs` are deleted (or the
      Tauri one reduced to a resolver adapter with no backoff array).
- [ ] The CLI daemon supervisor no longer uses `while let Ok(..)` on the
      connection-established receiver (plan-003 lag fix folded in):
      `git grep -n "while let Ok" crates/splitter-cli/src/commands/daemon/mod.rs`
      returns nothing for that loop.
- [ ] No files outside the in-scope list are modified (`git status`).
- [ ] `plans/README.md` updated: 020 DONE; 003 (and 005/015 if present) marked
      REJECTED with "folded into 020".

## STOP conditions

Stop and report back (do not improvise) if:

- **The two frontends' behavior differs in a way the observer cannot express
  without regressing one of them.** Specifically: if handling a message requires
  Tauri-only state (e.g. `remote_devices`, `peers`) that the CLI has no analog
  for, and you cannot model it as an optional observer hook with a CLI no-op —
  report the exact message and the divergence for a human decision. Do NOT
  silently pick one frontend's behavior for both.
- Converging a policy divergence (SetMuted mirroring, or SessionRequest
  evict-vs-dedup) would change user-visible behavior in a way that isn't clearly
  the safer/intended choice — report both options and stop rather than deciding
  unilaterally.
- Moving the loop into core requires a **new public API on `SignalingServer` /
  `connection.rs`** beyond a trivial read accessor — confirm the API shape before
  adding it.
- Plans 018 and 019 are not both DONE and green at start — this plan has no
  safety net without them; stop.
- A characterization test from 018/019 goes red for a reason other than the two
  documented CLI convergences — the extraction changed behavior; diagnose and fix
  the extraction, do not edit the test.
- The cited line ranges do not match the live code (drift), especially if plan
  003 already rewrote the CLI supervisor.

## Maintenance notes

- After this lands, **all** signaling control-plane changes happen once, in
  `crates/splitter-core/src/net/signaling/control_plane.rs`; both frontends are
  observers. A reviewer should verify no logic crept back into `acceptor.rs` or
  `peer_event_loop.rs` beyond observer callbacks and a thin spawner.
- A reviewer should diff the preserved user-facing strings/events carefully:
  every CLI `>> ...` line and every Tauri emitted event type must survive
  verbatim.
- The observer trait is the extension point for a future third frontend (e.g. a
  headless service) — keep its methods narrow and effect-only.
- Follow-up explicitly deferred: unifying the *outgoing* stream-open path
  (`open_stream_core` in Tauri vs the CLI's `send`/`stream_repl` commands) — this
  plan unifies the inbound/acceptor plane only. Note it for a later plan.
</content>
