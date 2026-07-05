# Plan 017: Split the 1498-line `stream_runtime.rs` god-module into focused siblings

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` if that file exists (it may not yet — if absent, skip).
>
> **Drift check (run first)**:
> `git diff --stat 217a31d..HEAD -- crates/splitter-core/src/net/stream_runtime.rs crates/splitter-core/src/net/mod.rs crates/splitter-core/src/lib.rs`
> This plan is a pure code MOVE with **zero behavior change**. Because plan 016
> rewrites the two pump bodies in this same file, the exact line numbers below
> WILL shift once 016 lands — that is expected. Re-map the boundaries by symbol
> name (not line number) against the live file before moving anything. A
> mismatch in *structure* (a symbol named here no longer exists) is a STOP
> condition; shifted line numbers are not.

## Status

- **Priority**: P2
- **Effort**: M
- **Risk**: MED
- **Depends on**: plans/016-wire-jitterbuffer-and-fec-into-session-path.md
  (do 016 first, so this plan moves the already-wired pumps and avoids a churn
  conflict on the pump bodies). If characterization-test plans 018/019 exist,
  do those first too.
- **Category**: tech-debt
- **Planned at**: commit `217a31d`, 2026-07-05

## Why this matters

`crates/splitter-core/src/net/stream_runtime.rs` is 1498 lines — the largest
file in the workspace — and mixes at least six unrelated responsibilities:
control-signal plumbing, an `unsafe impl Send/Sync for DeviceGuard`, stream
statistics + snapshots, the `StreamRegistry` map, the two real-time pump loops,
five `open_stream_as_*` orchestration functions, and the device hot-plug
dispatcher. Roughly two-thirds of the file is `#[cfg(test)]`. This makes the
file hard to navigate, hard to review (a pump change and a registry change land
in the same diff), and it slows incremental compiles because any edit
recompiles the whole module.

This plan mechanically extracts three cohesive concerns into sibling modules
under `net/` — statistics, the registry, and the pumps — with **no behavior
change whatsoever**. The public API surface is preserved exactly via re-exports
from `stream_runtime.rs`, so every external caller (`src-tauri`,
`splitter-cli`, `splitter-integration-tests`, and the crate-root re-export in
`lib.rs`) keeps compiling untouched. Success is: the entire existing test suite
stays green and the public paths are byte-for-byte identical.

## Current state

### The file's structure (map by symbol; line numbers are pre-016)

| Region | Symbols | Approx lines (pre-016) |
|--------|---------|------------------------|
| Imports + seq helper | `SEQ_MASK`, `seq_gap` | 1–24 |
| Control | `ControlOutcome`, `apply_control`, `StreamControlSignal`, `From<StreamAction>` | 26–78 |
| Device/runtime | `DeviceGuard` + `unsafe impl Send/Sync`, `StreamRuntime`, `StreamRuntime::abort` | 80–130 |
| Stats | `StreamStats`, `StreamStatsSnapshot`, `StreamStats::snapshot` | 132–182 |
| (tests) | `runtime_tests` (control-signal channel) | 184–205 |
| Registry | `StreamRuntimeSummary`, `StreamRegistry` + impl | 207–319 |
| (tests) | `registry_tests` | 321–428 |
| (tests) | `tests` (snapshot/seq_gap/apply_control) | 430–551 |
| Pumps | `spawn_source_pump_inner`, `spawn_sink_pump_inner`, `apply_gain_and_push` | 553–723 |
| (tests) | `sink_pump_tests` | 725–832 |
| (tests) | `source_pump_tests` | 834–934 |
| Orchestration helpers | `bind_and_connect_udp`, `build_runtime` (+ `use AudioRing`, `use StreamRoute`) | 936–973 |
| Orchestration | `open_stream_as_sink_inproc`, `open_stream_as_source_inproc`, `SourceKind`, `open_stream_as_source`, `open_stream_as_sink` | 975–1174 |
| (tests) | `open_sink_tests`, `open_source_tests`, `session_registration_failure_tests` | 1176–1422 |
| Hot-plug | `dispatch_device_events` (+ `use DeviceEvent`, `use broadcast`) | 1424–1458 |
| (tests) | `hotplug_tests` | 1460–1498 |

### Public surface that MUST stay importable from `net::stream_runtime`

External code imports these names from `crate::net::stream_runtime` /
`splitter_core::net::stream_runtime`. After the split, `stream_runtime.rs` must
re-export every one of them so those imports keep resolving. Verified callers:

- `crates/splitter-core/src/lib.rs:13-16` re-exports:
  `StreamControlSignal, StreamRegistry, StreamRuntime, StreamRuntimeSummary,
  StreamStats, StreamStatsSnapshot`.
- `crates/splitter-core/src/net/signaling/heartbeat.rs:37` imports:
  `DeviceGuard, StreamControlSignal, StreamRegistry, StreamRuntime, StreamStats`
  (and uses `StreamRegistry::snapshot_stats`).
- `crates/splitter-cli/*` and `src-tauri/*` import:
  `open_stream_as_sink, open_stream_as_source, SourceKind,
  StreamControlSignal, StreamRegistry, dispatch_device_events`.
- `crates/splitter-core/tests/stream_data_plane.rs`,
  `crates/splitter-integration-tests/tests/audio_rms_sustained.rs`,
  `crates/splitter-integration-tests/tests/soak.rs` import:
  `spawn_sink_pump_inner, spawn_source_pump_inner, StreamControlSignal,
  StreamStats`.

So the full re-export list is: `StreamControlSignal`, `DeviceGuard`,
`StreamRuntime`, `StreamRuntimeSummary`, `StreamRegistry`, `StreamStats`,
`StreamStatsSnapshot`, `spawn_source_pump_inner`, `spawn_sink_pump_inner`,
`SourceKind`, `open_stream_as_sink`, `open_stream_as_source`,
`open_stream_as_sink_inproc`, `open_stream_as_source_inproc`,
`dispatch_device_events`.

### `net/mod.rs` today (`crates/splitter-core/src/net/mod.rs`)

```rust
pub mod device_watcher;
pub mod discovery;
pub mod fec;
pub(crate) mod fs_util;
pub mod identity;
pub mod jitter;
pub mod manager;
pub mod packet;
pub mod session;
pub mod signaling;
pub mod stream;
pub mod stream_runtime;
pub mod trust;
```

### Cross-module visibility notes

- `StreamControlSignal` is used by the pumps (`apply_control`), the registry
  (`send_control`, `close`), the hot-plug dispatcher, AND external callers — it
  is the shared vocabulary. Keep its definition in `stream_runtime.rs` and let
  the new sibling modules import it via `use crate::net::stream_runtime::StreamControlSignal;`.
- `StreamRegistry::inner` is `pub(crate)` (mod.rs field at line 216) and is
  accessed by `dispatch_device_events` (same file today) and by
  `registry_tests`. After the move, `dispatch_device_events` stays in
  `stream_runtime.rs` and must still reach `inner` — keep the field
  `pub(crate)` so a sibling module can read it.
- `StreamStats` is used by the registry, the pumps, and `StreamRuntime`.
- `apply_control` / `ControlOutcome` / `seq_gap` / `SEQ_MASK` are used only by
  the pumps — move them alongside the pumps.
- `build_runtime` / `bind_and_connect_udp` are used only by the `open_stream_*`
  functions — keep them in `stream_runtime.rs`.

## Target module layout

Create three new sibling files under `crates/splitter-core/src/net/` and slim
`stream_runtime.rs` to control + device/runtime + orchestration + hot-plug +
re-exports:

1. **`net/stream_stats.rs`** — `StreamStats`, `StreamStatsSnapshot`,
   `StreamStats::snapshot`, and the `tests` items that exercise ONLY stats
   (`fresh_stats_snapshot_is_all_zero`, `snapshot_computes_bitrate_from_window`,
   `snapshot_reads_rtt_atomically`, `stats_snapshot_default_is_all_zeros`).

2. **`net/stream_registry.rs`** — `StreamRuntimeSummary`, `StreamRegistry` +
   full impl, and `registry_tests`. Imports `StreamRuntime`, `StreamStats`,
   `StreamStatsSnapshot`, `StreamControlSignal` from siblings.

3. **`net/stream_pump.rs`** — `SEQ_MASK`, `seq_gap`, `ControlOutcome`,
   `apply_control`, `spawn_source_pump_inner`, `spawn_sink_pump_inner`,
   `apply_gain_and_push`, plus `sink_pump_tests`, `source_pump_tests`, and the
   seq_gap / apply_control unit tests. (After 016, this is where the jitter/FEC
   wiring lives — move it verbatim.)

4. **`net/stream_runtime.rs`** (slimmed) — keeps `StreamControlSignal` +
   `From<StreamAction>`, `DeviceGuard` + `unsafe impl`, `StreamRuntime` +
   `abort`, `bind_and_connect_udp`, `build_runtime`, all five `open_stream_as_*`
   + `SourceKind`, `dispatch_device_events`, the `runtime_tests`,
   `open_sink_tests`, `open_source_tests`, `session_registration_failure_tests`,
   `hotplug_tests`, AND the `pub use` re-exports of everything moved out.

Register the new modules in `net/mod.rs`. Make them `pub mod` (simplest and
harmless — nothing outside the crate imports these new paths, and the public
API is preserved by the re-exports regardless):

```rust
pub mod stream_pump;
pub mod stream_registry;
pub mod stream_runtime;
pub mod stream_stats;
```

At the top of the slimmed `stream_runtime.rs`, re-export the moved public items
so external imports keep resolving:

```rust
pub use crate::net::stream_pump::{spawn_sink_pump_inner, spawn_source_pump_inner};
pub use crate::net::stream_registry::{StreamRegistry, StreamRuntimeSummary};
pub use crate::net::stream_stats::{StreamStats, StreamStatsSnapshot};
```

(`StreamControlSignal`, `DeviceGuard`, `StreamRuntime`, `SourceKind`,
`open_stream_as_*`, `dispatch_device_events` stay defined in
`stream_runtime.rs`, so no re-export needed for them.)

## Commands you will need

| Purpose   | Command                                                     | Expected on success |
|-----------|------------------------------------------------------------|---------------------|
| Build     | `cargo build --workspace`                                  | exit 0              |
| Tests     | `cargo test --workspace`                                   | all pass, same count as before |
| Core only | `cargo test -p splitter-core`                              | all pass            |
| Lint      | `cargo clippy --workspace --all-targets -- -D warnings`    | exit 0, no warnings |
| Format    | `cargo fmt --all -- --check`                               | exit 0, no diff     |

## Scope

**In scope** (the only files you should modify or create):
- `crates/splitter-core/src/net/stream_runtime.rs` (slim down + re-exports)
- `crates/splitter-core/src/net/stream_stats.rs` (create)
- `crates/splitter-core/src/net/stream_registry.rs` (create)
- `crates/splitter-core/src/net/stream_pump.rs` (create)
- `crates/splitter-core/src/net/mod.rs` (register new modules)

**Out of scope** (do NOT touch — the re-exports must make edits here
unnecessary):
- `crates/splitter-core/src/lib.rs` — the crate-root re-export stays as-is.
- `signaling/heartbeat.rs`, `src-tauri/*`, `splitter-cli/*`,
  `splitter-integration-tests/*` — must compile unchanged.
- Any behavior, signature, visibility (`pub`/`pub(crate)`) of a moved item —
  preserve exactly.
- `jitter.rs`, `fec.rs`, `codec.rs`.

## Git workflow

- Branch: `advisor/017-split-stream-runtime`
- Conventional-commit **title only**, no body, e.g.
  `refactor(net): split stream_runtime into stats/registry/pump modules`.
- **NEVER** add a `Co-Authored-By` trailer.
- Move in small commits (one module per commit is ideal for review) so each
  intermediate state still builds. Do NOT push or open a PR.

## Steps

Order the moves so the crate compiles after each step. Move leaf modules first
(stats has no dependency on registry/pump), then registry (depends on stats +
runtime), then pumps.

### Step 1: Extract `stream_stats.rs`

- Create `net/stream_stats.rs`. Move `StreamStats`, `StreamStatsSnapshot`,
  `impl StreamStats { snapshot }`, and the four stats-only `#[test]` fns.
- Add needed imports at the top (`use serde::Serialize;`,
  `use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};`). Move the
  stats-only tests into a `#[cfg(test)] mod tests` in the new file.
- Add `pub mod stream_stats;` to `net/mod.rs`.
- In `stream_runtime.rs`, delete the moved definitions and add
  `pub use crate::net::stream_stats::{StreamStats, StreamStatsSnapshot};`.
  Fix any now-unused imports it leaves behind.

**Verify**: `cargo test -p splitter-core` → all pass (same test count minus
none — the moved tests now run from the new module).

### Step 2: Extract `stream_registry.rs`

- Create `net/stream_registry.rs`. Move `StreamRuntimeSummary`,
  `StreamRegistry` + its full impl, and `registry_tests`.
- Imports: `StreamRuntime`, `StreamControlSignal` from
  `crate::net::stream_runtime`; `StreamStats`, `StreamStatsSnapshot` from
  `crate::net::stream_stats` (or via the `stream_runtime` re-export — either
  resolves; prefer the direct sibling path); plus `NetError`, `SessionId`,
  `StreamId`, `HashMap`, `RwLock`, `Arc`.
- Keep `StreamRegistry::inner` as `pub(crate)` (the hot-plug dispatcher in
  `stream_runtime.rs` reads it across the module boundary — `pub(crate)` makes
  that legal).
- Add `pub mod stream_registry;` to `net/mod.rs`.
- In `stream_runtime.rs`, delete the moved definitions, add
  `pub use crate::net::stream_registry::{StreamRegistry, StreamRuntimeSummary};`.

**Verify**: `cargo test -p splitter-core` → all pass.

### Step 3: Extract `stream_pump.rs`

- Create `net/stream_pump.rs`. Move `SEQ_MASK`, `seq_gap`, `ControlOutcome`,
  `apply_control`, `spawn_source_pump_inner`, `spawn_sink_pump_inner`,
  `apply_gain_and_push`, and the tests: `sink_pump_tests`, `source_pump_tests`,
  and the seq_gap/apply_control `#[test]` fns from the old `tests` mod.
- Imports: `StreamControlSignal` from `crate::net::stream_runtime`;
  `StreamStats` from `crate::net::stream_stats`; the codec, ring, packet,
  jitter, fec, settings, and `FRAME_*` items the pumps use (copy the exact
  `use` lines the pump code references). After 016 these include
  `crate::net::jitter::{JitterBuffer, JitterOutput}`,
  `crate::net::fec::FecController`, and `crate::settings::{FecMode, JitterMode}`.
- Add `pub mod stream_pump;` to `net/mod.rs`.
- In `stream_runtime.rs`, delete the moved definitions, add
  `pub use crate::net::stream_pump::{spawn_sink_pump_inner, spawn_source_pump_inner};`.
- Note: `apply_control`, `ControlOutcome`, `seq_gap`, `SEQ_MASK`,
  `apply_gain_and_push` are private (`fn`, no `pub`) and used only by the
  pumps — they can stay private inside `stream_pump.rs` (no re-export needed).

**Verify**: `cargo test -p splitter-core` → all pass.

### Step 4: Confirm the slimmed `stream_runtime.rs` and clean up

- The remaining `stream_runtime.rs` holds control (`StreamControlSignal`,
  `From`), `DeviceGuard` + `unsafe impl`, `StreamRuntime` + `abort`,
  `bind_and_connect_udp`, `build_runtime`, `SourceKind`, all `open_stream_as_*`,
  `dispatch_device_events`, the three re-export lines, and the tests
  `runtime_tests`, `open_sink_tests`, `open_source_tests`,
  `session_registration_failure_tests`, `hotplug_tests`.
- Remove any imports that are now unused in `stream_runtime.rs` (e.g. codec,
  jitter, ring producer/consumer imports that moved with the pumps). Let clippy
  guide you.

**Verify**:
- `cargo build --workspace` → exit 0.
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0 (no
  `unused_imports`, no `dead_code`).

### Step 5: Full verification (behavior-preservation gate)

**Verify**:
- `cargo test --workspace` → all pass. The total number of tests must be the
  same as before the split (record the count from `git stash`ed baseline if
  unsure — moving a test does not change the count).
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0.
- `cargo fmt --all -- --check` → exit 0.
- `git diff 217a31d..HEAD -- crates/splitter-core/src/lib.rs` → empty (the
  crate-root re-export was NOT edited).

## Test plan

- No NEW tests. Every existing test MOVES with the code it exercises and must
  keep passing. The split is behavior-preserving.
- Guard: `cargo test --workspace` test count is unchanged vs. the baseline.
- Structural pattern for the new `#[cfg(test)] mod` blocks: keep each moved test
  module's name and body identical; only its file location changes.

## Done criteria

Machine-checkable. ALL must hold:

- [ ] `cargo build --workspace` exits 0
- [ ] `cargo test --workspace` exits 0, same test count as pre-split baseline
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` exits 0
- [ ] `cargo fmt --all -- --check` exits 0
- [ ] `crates/splitter-core/src/net/{stream_stats,stream_registry,stream_pump}.rs`
      all exist and are registered in `net/mod.rs`
- [ ] `wc -l crates/splitter-core/src/net/stream_runtime.rs` is materially
      smaller (target: well under half the original 1498)
- [ ] `git diff 217a31d..HEAD -- crates/splitter-core/src/lib.rs` is empty
- [ ] No file under `src-tauri/`, `crates/splitter-cli/`, or
      `crates/splitter-integration-tests/` was modified (`git status`)
- [ ] `plans/README.md` status row updated (if that file exists)

## STOP conditions

Stop and report back (do not improvise) if:

- A symbol named in "Current state" no longer exists in the live file in a
  recognizable form (structural drift — as opposed to merely shifted line
  numbers, which are expected after 016).
- A split forces a **public API change** that ripples beyond the re-exports —
  e.g. an external file (`src-tauri`, `splitter-cli`,
  `splitter-integration-tests`, `signaling/heartbeat.rs`, `lib.rs`) stops
  compiling and cannot be fixed by adding a `pub use` in `stream_runtime.rs`.
  Report before editing any out-of-scope file.
- You find you must change the visibility of a moved item from what it was
  (e.g. a `pub(crate)` field would have to become `pub`) to make the split
  compile — report the specific item.
- The workspace test count changes (a test was dropped or duplicated during the
  move).
- Plan 016 has NOT landed yet and the pump bodies still contain the old
  `last_seq` logic (that means you're about to move soon-to-be-rewritten code —
  do 016 first).

## Maintenance notes

- **Reviewer scrutiny:** this must be a pure move. Review with
  `git diff -M` (rename detection) and confirm no logic changed — only
  locations and the added `pub use` / `use` lines. Confirm the `unsafe impl
  Send/Sync for DeviceGuard` SAFETY comment moved intact with `DeviceGuard`.
- **Future interaction:** once split, a follow-up could make the new sibling
  modules `pub(crate)` instead of `pub` and drop the redundant `pub use` layer,
  but only after auditing that nothing depends on the `net::stream_runtime::*`
  paths — deferred here to keep this change zero-risk.
- If plan 016's deferred "thread SettingsHandle through orchestration" follow-up
  lands later, the config-plumbing will live in `stream_runtime.rs`
  (orchestration) and be passed into `stream_pump.rs` — keep that boundary in
  mind.
</content>
