# Plan 008: Make the jitter buffer's in-order pop O(1) in the common case and add a hard packet cap

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat 217a31d..HEAD -- crates/splitter-core/src/net/jitter.rs`
> If the file changed since this plan was written, compare the "Current state"
> excerpts against the live code before proceeding; on a mismatch, treat it as a
> STOP condition.

## Status

- **Priority**: P2
- **Effort**: S
- **Risk**: MED
- **Depends on**: none
- **Category**: perf
- **Planned at**: commit `217a31d`, 2026-07-05

## Why this matters

`JitterBuffer` sits on the receive path and runs `pop_ready` once per playback
tick. Two issues:

1. **`pop_ready` scans and shifts the whole `arrival_order` `VecDeque` on every
   in-order pop.** `self.arrival_order.retain(|&s| s != want)` (jitter.rs:95) walks
   the entire deque and shifts elements even though we know which seq we just
   popped. In the steady in-order case the popped seq is the front element, so this
   should be O(1) but is O(n).
2. **`queue` and `arrival_order` have no hard size cap.** They grow on every `push`
   (jitter.rs:78-81) and are only bounded *when `pop_ready` is called* via age-based
   eviction. If the consumer stalls (stops calling `pop_ready`), both structures
   grow **unbounded** — a memory-growth footgun. Contrast `arrival_intervals_ms`,
   which is explicitly capped at 256 (jitter.rs:70-73).

The fix makes the common in-order pop O(1) while **preserving correctness for
out-of-order arrival**, and adds a hard packet-count cap that drops the oldest
buffered packet when exceeded, bounding memory regardless of consumer behavior.

### CRITICAL correctness note — do NOT blindly `pop_front`

`arrival_order` is ordered by **arrival**, not by **seq**. `pop_ready` pops by
`next_expected_seq` (seq order). Under reordering these differ, so the popped seq
is *not always* the front of `arrival_order`. A blind `pop_front()` would corrupt
`arrival_order` and break age-gating. Two existing tests prove this and are your
guard:

- `reorders_out_of_order_arrival` (jitter.rs:175) pushes seq 2,0,1 → `arrival_order`
  is `[2,0,1]` but the first in-order pop is seq 0 (front is 2).
- `age_gating_uses_oldest_arrival_not_lowest_seq` (jitter.rs:245) depends on
  `arrival_order.front()` still pointing at the *oldest-arrived* seq after an
  in-order pop.

The correct optimization is: **if the front equals `want`, `pop_front()` (O(1));
otherwise fall back to `retain` (O(n))**. In-order steady state hits the fast path;
reordering keeps correct behavior.

## Current state

File: `crates/splitter-core/src/net/jitter.rs` (single struct, ~283 lines).

Struct (jitter.rs:15-26):

```rust
pub struct JitterBuffer {
    mode: JitterMode,
    max_depth_ms: u32,
    target_depth: usize,
    next_expected_seq: Option<u32>,
    queue: BTreeMap<u32, (Packet, Instant)>,
    arrival_order: VecDeque<u32>,
    arrival_intervals_ms: VecDeque<u32>,
    last_arrival: Option<Instant>,
    pops_since_resize: u32,
}
```

`push` — unbounded growth (jitter.rs:67-89):

```rust
pub fn push(&mut self, packet: Packet, arrival: Instant) {
    if let Some(prev) = self.last_arrival {
        let delta = arrival.duration_since(prev).as_millis() as u32;
        if self.arrival_intervals_ms.len() >= 256 {
            self.arrival_intervals_ms.pop_front();
        }
        self.arrival_intervals_ms.push_back(delta);
    }
    self.last_arrival = Some(arrival);
    let seq = packet.seq;
    let is_new = !self.queue.contains_key(&seq);
    self.queue.insert(seq, (packet, arrival));
    if is_new {
        self.arrival_order.push_back(seq);
    }
    match self.next_expected_seq {
        None => self.next_expected_seq = Some(seq),
        Some(cur) if seq < cur => self.next_expected_seq = Some(seq),
        _ => {}
    }
}
```

`pop_ready` — the O(n) `retain` (jitter.rs:91-113):

```rust
pub fn pop_ready(&mut self, now: Instant) -> Option<JitterOutput> {
    let want = self.next_expected_seq?;
    if self.queue.contains_key(&want) {
        let (pkt, _) = self.queue.remove(&want)?;
        self.arrival_order.retain(|&s| s != want);   // <-- O(n) scan + shift
        self.next_expected_seq = Some(want.wrapping_add(1));
        self.bump_pops();
        return Some(JitterOutput::Packet(pkt));
    }
    if self.queue.is_empty() {
        return None;
    }
    let oldest_seq = *self.arrival_order.front()?;
    let oldest_arrival = self.queue.get(&oldest_seq).map(|(_, t)| *t)?;
    let age_ms = now.duration_since(oldest_arrival).as_millis() as u32;
    if age_ms >= self.max_depth_ms {
        let lost = JitterOutput::Lost { seq: want };
        self.next_expected_seq = Some(want.wrapping_add(1));
        self.bump_pops();
        return Some(lost);
    }
    None
}
```

Constants already present (jitter.rs:6-7):

```rust
pub const PACKET_INTERVAL_MS: u32 = 20;
pub const MAX_DEPTH_MS_HARD_CAP: u32 = 200;
```

At 20 ms/packet and a 200 ms hard cap, an in-spec buffer holds ~10 packets; a cap
of a few hundred is far above any legitimate depth while still bounding memory.

### Conventions

- **No code comments** except a non-obvious *why* (project rule in `CLAUDE.md`). The
  reorder-vs-arrival-order subtlety **is** a legitimate *why* — a one-line comment
  explaining the front-check fast path is acceptable and encouraged.
- Tests live in `#[cfg(test)] mod tests` at the bottom (jitter.rs:144-282); the
  `pkt(seq)` helper (jitter.rs:150-157) builds a `Packet`. Model new tests on
  `pops_in_seq_order_when_ordered` and `age_gating_uses_oldest_arrival_not_lowest_seq`.
- `JitterOutput` is `Packet(Packet)` | `Lost { seq }`.

## Commands you will need

| Purpose | Command                                                         | Expected on success |
|---------|-----------------------------------------------------------------|---------------------|
| Build   | `cargo build --workspace`                                       | exit 0              |
| Tests   | `cargo test -p splitter-core net::jitter`                       | all pass            |
| Tests   | `cargo test --workspace`                                        | all pass            |
| Lint    | `cargo clippy --workspace --all-targets -- -D warnings`         | exit 0              |
| Format  | `cargo fmt --all -- --check`                                    | exit 0              |

## Scope

**In scope** (the only file you should modify):
- `crates/splitter-core/src/net/jitter.rs`

**Out of scope** (do NOT touch):
- The public API surface `push` / `pop_ready` / `target_depth_packets` /
  `p99_jitter_ms` / `JitterOutput` signatures — callers depend on them; you are
  changing internals only.
- The age-gating semantics (SAFETY of playback timing) — `pop_ready` must still emit
  `Lost { seq }` at exactly the same moments; `age_gating_uses_oldest_arrival_not_lowest_seq`
  is the guard.
- The `arrival_intervals_ms` cap logic and `recompute_target` / `bump_pops` — leave
  them as-is.
- Any other file in `net/` — the jitter buffer is self-contained.

## Git workflow

- Branch: `advisor/008-jitterbuffer-on-scan-and-cap`
- Commit style: conventional-commit **title only**, no body. Example fitting title:
  `perf(net): O(1) jitter in-order pop + hard packet cap`.
- **NEVER** add a `Co-Authored-By` trailer of any kind.
- Do NOT push or open a PR.

## Steps

### Step 1: Add a hard packet-count cap constant

Near the existing constants (jitter.rs:6-7), add:

```rust
// WHY: queue/arrival_order only shrink when pop_ready runs; a stalled consumer
// would otherwise grow them unbounded. At 20ms/packet and a 200ms hard cap an
// in-spec buffer holds ~10 packets, so 512 is far above any legitimate depth while
// still bounding memory.
pub const MAX_QUEUED_PACKETS: usize = 512;
```

**Verify**: `cargo build --workspace` → exit 0.

### Step 2: Enforce the cap in `push` (drop oldest arrival when exceeded)

At the end of `push`, after inserting the new packet and updating
`next_expected_seq`, evict the oldest-arrived packet(s) until within the cap. Drop
by **arrival order** (`arrival_order.front()`), removing the same seq from `queue`:

```rust
while self.arrival_order.len() > MAX_QUEUED_PACKETS {
    if let Some(dropped) = self.arrival_order.pop_front() {
        self.queue.remove(&dropped);
    } else {
        break;
    }
}
```

Place this as the final block of `push`. Do not touch `next_expected_seq` here: if
the dropped packet was the one we were waiting for, the normal age-gating path in
`pop_ready` already emits `Lost` for it — dropping it from storage only bounds
memory, it does not change the loss decision.

Rationale for evicting the front of `arrival_order`: it is the oldest *arrival*,
which is the most stale and the least likely to still be playable within the depth
window.

**Verify**: `cargo build --workspace` → exit 0.

### Step 3: Make the in-order pop O(1) in the common case

In `pop_ready`, replace the unconditional `retain` (jitter.rs:95) with a front-check
fast path and a `retain` fallback:

```rust
if self.queue.contains_key(&want) {
    let (pkt, _) = self.queue.remove(&want)?;
    // arrival_order is arrival-ordered, not seq-ordered: the popped seq is the
    // front only in the in-order steady state. Fast-path that; fall back to a
    // scan when a reordered arrival put `want` elsewhere.
    if self.arrival_order.front() == Some(&want) {
        self.arrival_order.pop_front();
    } else {
        self.arrival_order.retain(|&s| s != want);
    }
    self.next_expected_seq = Some(want.wrapping_add(1));
    self.bump_pops();
    return Some(JitterOutput::Packet(pkt));
}
```

Do **not** change any other branch of `pop_ready`.

**Verify**: `cargo test -p splitter-core net::jitter` → all existing tests pass,
especially `reorders_out_of_order_arrival` (jitter.rs:175) and
`age_gating_uses_oldest_arrival_not_lowest_seq` (jitter.rs:245). If either fails,
you took the fast path when you should have taken the fallback — STOP.

### Step 4: Full verification

- `cargo test --workspace`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo fmt --all -- --check`

**Verify**: all three exit 0.

## Test plan

Add to `#[cfg(test)] mod tests` (use the existing `pkt(seq)` helper):

- `in_order_pop_removes_only_matching_head`: push seq 0,1,2 in order; pop once;
  assert the returned packet is seq 0 **and** the remaining buffer still pops seq 1
  then 2 in order (proves the front fast path removed exactly the popped seq). This
  is the O(1)-shape guard.
- `out_of_order_pop_preserves_arrival_order_for_age_gating`: reproduce the shape of
  `age_gating_uses_oldest_arrival_not_lowest_seq` but assert the reorder fallback
  path is exercised — push seq 10 (t0), 7 (t0+20ms), 4 (t0+40ms); pop seq 4 (front
  of arrival_order is 10, so fallback runs); then at t0+60ms with `max_depth_ms=50`
  assert `Some(JitterOutput::Lost { seq: 5 })`. (This is essentially the existing
  test; adding it under a name that documents the fallback is fine, or rely on the
  existing test — but explicitly confirm it still passes.)
- `push_beyond_cap_drops_oldest`: with a fresh `JitterBuffer::new(JitterMode::Min, 100)`,
  push `MAX_QUEUED_PACKETS + 5` packets with increasing seq (all at the same
  `Instant` is fine). Assert `arrival_order.len() == MAX_QUEUED_PACKETS` and that the
  oldest-arrived seqs (the first 5 pushed) are no longer in `queue`
  (`assert!(!jb.queue.contains_key(&0))` etc.). Tests are in the same module, so they
  can read the private `arrival_order`/`queue` fields directly.
- `cap_does_not_break_in_order_playback_within_cap`: push exactly
  `MAX_QUEUED_PACKETS` in-order packets, then pop them all and assert they come out
  in seq order 0..MAX_QUEUED_PACKETS (nothing dropped when at/under the cap).

Model structure on `pops_in_seq_order_when_ordered` (jitter.rs:159) and
`age_gating_uses_oldest_arrival_not_lowest_seq` (jitter.rs:245).

Verification: `cargo test -p splitter-core net::jitter` → all pass, new tests
included.

## Done criteria

Machine-checkable. ALL must hold:

- [ ] `cargo build --workspace` exits 0
- [ ] `cargo test --workspace` exits 0; the new cap + fast-path tests exist and pass
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` exits 0
- [ ] `cargo fmt --all -- --check` exits 0
- [ ] `grep -n "MAX_QUEUED_PACKETS" crates/splitter-core/src/net/jitter.rs` shows the
      const defined and used in `push`
- [ ] Both `reorders_out_of_order_arrival` and
      `age_gating_uses_oldest_arrival_not_lowest_seq` still pass unchanged
- [ ] No files outside `crates/splitter-core/src/net/jitter.rs` are modified
      (`git status`)
- [ ] `plans/README.md` status row for 008 updated

## STOP conditions

Stop and report back (do not improvise) if:

- The drift check shows `jitter.rs` changed and the "Current state" excerpts no
  longer match.
- `reorders_out_of_order_arrival` or `age_gating_uses_oldest_arrival_not_lowest_seq`
  fails after Step 3 — this means the fast path is being taken when the seq is not at
  the front. Do NOT weaken those tests; the fix is the `front() == Some(&want)` guard.
- Enforcing the cap changes the outcome of any existing test (it should not — the cap
  is far above the depths those tests use).
- You conclude the cap needs to also adjust `next_expected_seq` to stay correct —
  stop and report, because that would change loss semantics and is out of the intended
  scope.

## Maintenance notes

- If the packet interval or `MAX_DEPTH_MS_HARD_CAP` changes materially, revisit
  whether `MAX_QUEUED_PACKETS = 512` is still comfortably above the legitimate depth.
- The front-check fast path assumes `arrival_order` stays arrival-ordered. Any future
  change that reorders `arrival_order` (e.g. sorting it by seq) would invalidate both
  the fast path and the age-gating `front()` lookup — treat `arrival_order`'s ordering
  as an invariant.
- Reviewer scrutiny: confirm `pop_ready`'s `Lost` timing is byte-identical (age-gating
  test), and confirm the cap eviction removes from **both** `queue` and `arrival_order`
  (a leak in either reintroduces unbounded growth or a dangling `arrival_order` entry).
