# Plan 011: Drop or lengthen redundant snapshot/pending polling intervals

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat 217a31d..HEAD -- src/hooks/useSnapshot.ts src/hooks/usePeers.ts src/lib/events.ts src/app/queryClient.ts src-tauri/src/acceptor.rs src-tauri/src/commands/streams.rs`
> If any in-scope file changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Priority**: P2
- **Effort**: S
- **Risk**: MED
- **Depends on**: none
- **Category**: perf
- **Planned at**: commit `217a31d`, 2026-07-05

## Why this matters

Two React Query queries are **both** event-invalidated **and** interval-polled,
so they refetch on a timer even when nothing changed:

- `snapshot` polls every **3000 ms** (`src/hooks/useSnapshot.ts:8`), and is
  already invalidated by three separate Tauri events in `src/lib/events.ts`
  (`snapshotChanged`, `incomingSession`, `peerDisconnected`).
- `pending` polls every **1500 ms** (`src/hooks/usePeers.ts:14`), and is already
  invalidated by `incomingSession` in `src/lib/events.ts:16`.

On an idle desktop app that is ~20 snapshot refetches/min + ~40 pending
refetches/min ≈ **60 redundant round-trips/min**, each an IPC call into the Rust
core, for a UI that an event system already keeps current. `staleTime` is only
2000 ms (`src/app/queryClient.ts:5`), so the intervals genuinely refetch.

**This is only safe if the event stream covers every state transition.** If some
transition has no event, the poll is the *only* thing that eventually surfaces it,
and removing the poll would make the UI silently stale. Step 1 verifies coverage
before anything is removed; Step 2 is conditional on that result. The safe
default when coverage is uncertain is a **15 s low-frequency backstop**, not full
removal.

## Current state

`src/hooks/useSnapshot.ts` (whole file):

```ts
export const useSnapshot = () =>
  useQuery({
    queryKey: ["snapshot"],
    queryFn: () => unwrap(commands.snapshot()),
    refetchInterval: 3000,           // line 8
  });
```

`src/hooks/usePeers.ts` (the pending query, lines 10-15):

```ts
export const usePendingPeers = () =>
  useQuery({
    queryKey: ["pending"],
    queryFn: () => unwrap(commands.pendingPeers()),
    refetchInterval: 1500,           // line 14
  });
```

(Note: `usePeers` — the `["peers"]` query at lines 4-8 — has **no** interval and
is out of scope. Only `usePendingPeers` polls.)

`src/lib/events.ts` — the event → invalidation bridge already in place:

```ts
events.peersChanged.listen(...)      // invalidates ["peers"]
events.incomingSession.listen(...)   // invalidates ["snapshot"] AND ["pending"]   (lines 15-16)
events.statsTick.listen(...)         // pushes stats to zustand
events.peerDisconnected.listen(...)  // invalidates ["snapshot"] and ["peers"]     (lines 20-21)
events.snapshotChanged.listen(...)   // invalidates ["snapshot"] and ["peerDevices"] (lines 24-25)
```

`src/app/queryClient.ts` — `staleTime: 2000, refetchOnWindowFocus: false, retry: 1`.

### Backend emit sites (read-only cross-check — already surveyed during planning)

The Rust `SnapshotChanged` event is emitted from `src-tauri/src/acceptor.rs` at
these points (line numbers approximate — verify in Step 1):

- incoming session request handled (~line 103)
- opened a stream as sink, i.e. **remote** opened a stream to us (~line 224)
- received a `StreamControl` from remote — remote pause/resume/mute/volume (~line 296)
- remote closed the session (~line 318)
- received a remote `DeviceListResponse` (~line 339)
- handled a remote `StreamRequest` successfully (~line 382)

Plus `PeerDisconnected` is emitted on peer disconnect (~line 393).

**Local** user actions do NOT rely on `SnapshotChanged`: the frontend mutation
hooks invalidate `["snapshot"]` themselves on success —
`src/hooks/useStreams.ts` (`useOpenStream`, `useRequestStream`, `useCloseStream`,
`useStreamControl` all `invalidateQueries({ queryKey: ["snapshot"] })`) and
`src/hooks/useConnection.ts` (`useAcceptPending`, `useDisconnect`,
`useOpenSession`). So the local-action path is covered by mutation invalidation,
and the remote-action path is covered by `SnapshotChanged`/`PeerDisconnected`.

**The one genuinely uncertain gap** (this is the MED risk): state transitions that
are **not** driven by a signaling message or a local mutation — most notably a
stream flipping to `state: "error"` due to a transport/audio failure, or a
recovery back to `active`. During planning no `SnapshotChanged` emit was found
tied to such an internally-detected stream-state change. If that gap is real, a
stream can enter an error state with no event, and only the poll would surface it.
Step 1 confirms this; if unresolved, keep the 15 s backstop.

## Commands you will need

| Purpose        | Command                                                        | Expected on success |
|----------------|---------------------------------------------------------------|---------------------|
| Typecheck      | `npm run typecheck`                                           | exit 0, no errors   |
| Full test      | `npm test`                                                    | all pass            |
| Build          | `npm run build`                                               | exit 0              |
| Grep emits     | `grep -rn "SnapshotChanged" src-tauri/src`                   | lists emit sites    |
| Grep interval  | `grep -rn "refetchInterval" src/hooks`                       | shows what remains  |

## Scope

**In scope** (the only files you should modify):
- `src/hooks/useSnapshot.ts`
- `src/hooks/usePeers.ts`

**Read-only cross-check** (open to VERIFY coverage; do NOT modify):
- `src/lib/events.ts`
- `src/hooks/useStreams.ts`, `src/hooks/useConnection.ts` (mutation invalidations)
- `src-tauri/src/acceptor.rs`, `src-tauri/src/commands/streams.rs`,
  `src-tauri/src/events.rs`

**Out of scope** (do NOT touch):
- Any Rust source. This plan does **not** add backend events. If Step 1 finds a
  real coverage gap, the resolution here is a frontend backstop interval, and
  emitting the missing event is a **separate** follow-up (note it, don't do it).
- `src/app/queryClient.ts` — leave `staleTime`/`retry` as-is.
- The `["peers"]` query in `usePeers.ts` — it has no interval; don't add one.

## Git workflow

- Branch: `advisor/011-remove-redundant-polling`
- Commit style: conventional-commit **title only**, no body (see `git log`).
  Example: `perf(hooks): replace snapshot/pending polling with event-driven backstop`.
- **NEVER** add a `Co-Authored-By` trailer.
- Do NOT push or open a PR.

## Steps

### Step 1: Verify event coverage (GATE — decides Step 2)

Do NOT change any interval yet. Establish whether events + mutation invalidations
cover **every** state transition that the poll would otherwise catch.

1. Run `grep -rn "SnapshotChanged" src-tauri/src` and open each emit site in
   `src-tauri/src/acceptor.rs`. Confirm the six remote-driven transitions listed
   in "Current state" each still emit `SnapshotChanged`.
2. Confirm local mutations self-invalidate: `grep -n "invalidateQueries" src/hooks/useStreams.ts src/hooks/useConnection.ts` — every stream/session mutation must invalidate `["snapshot"]`; `useAcceptPending`/`incomingSession` must invalidate `["pending"]`.
3. **The decisive check**: search for any stream-state transition to `error` (or
   recovery) that is detected internally (not from a signaling message or a local
   command) and does **NOT** emit `SnapshotChanged`. Try:
   `grep -rn "StreamState::Error\|state = .*Error\|set_stream_state\|StreamState::Active" src-tauri/src`
   and inspect whether each such mutation is followed by a `core.emit(SnapshotChanged)`.

Decision:

- **If coverage is COMPLETE** (every transition either emits an event or is a
  self-invalidating local mutation) → proceed to Step 2A (remove intervals).
- **If ANY gap exists** (some transition has neither an event nor a mutation
  invalidation) → **STOP the removal path** and go to Step 2B (15 s backstop).
  Record the specific uncovered transition(s) in your final report so a
  follow-up can add the missing backend event.

**Verify**: you have written down, explicitly, which branch (2A or 2B) you are
taking and why. Do not proceed without that determination.

### Step 2A: Remove the intervals (only if Step 1 = COMPLETE)

- `src/hooks/useSnapshot.ts`: delete the `refetchInterval: 3000,` line.
- `src/hooks/usePeers.ts`: delete the `refetchInterval: 1500,` line from
  `usePendingPeers` (leave the query otherwise intact).

**Verify**: `grep -rn "refetchInterval" src/hooks` → no matches;
`npm run typecheck` → exit 0.

### Step 2B: Lengthen to a 15 s backstop (if Step 1 found ANY gap)

- `src/hooks/useSnapshot.ts`: change `refetchInterval: 3000,` → `refetchInterval: 15000,`.
- `src/hooks/usePeers.ts`: change `refetchInterval: 1500,` →
  `refetchInterval: 15000,` in `usePendingPeers`.

Add a one-line WHY comment above each interval explaining the backstop (this is
an allowed exception to the no-comment rule — a non-obvious deliberate
trade-off), e.g.:

```ts
// Backstop only: <transition X> emits no event yet; events drive normal updates. Remove once that event exists.
```

**Verify**: `grep -rn "refetchInterval: 15000" src/hooks` → 2 matches;
`npm run typecheck` → exit 0.

### Step 3: Run the suite

**Verify**: `npm test` → all pass; `npm run build` → exit 0.

## Test plan

No behavior is added, so no new assertions are strictly required, but you MUST
confirm the existing suite stays green after the interval change:

- `npm test` → all pass. Pay attention to any test in
  `src/hooks/hooks.test.tsx` or feature tests that asserts on refetch timing; if
  one exists and breaks, that is a real signal — report it (a test may encode an
  assumption about polling).
- Manual reasoning check (document in your report, not a code change): with
  intervals removed/lengthened, list the concrete user-visible flows and which
  event/mutation now keeps each current (remote mute → `SnapshotChanged`; local
  close → `useCloseStream` invalidation; incoming pairing → `incomingSession`
  invalidates `["pending"]`; etc.).

Verification: `npm test` → all pass.

## Done criteria

Machine-checkable. ALL must hold:

- [ ] `npm run typecheck` exits 0
- [ ] `npm test` exits 0 (no test regressions)
- [ ] `npm run build` exits 0
- [ ] Exactly one branch taken: EITHER `grep -rn "refetchInterval" src/hooks`
      returns no matches (2A) OR it returns exactly the two 15000 backstops (2B)
- [ ] No Rust files modified; no files outside `src/hooks/` modified (`git status`)
- [ ] The Step 1 coverage determination (2A vs 2B, with reasoning and any
      uncovered transition) is in the final report
- [ ] `plans/README.md` status row for 011 updated

## STOP conditions

Stop and report back (do not improvise) if:

- The code at the locations in "Current state" doesn't match the excerpts, or the
  `SnapshotChanged` emit sites in `acceptor.rs` differ materially from the list.
- Step 1 is **inconclusive** — you cannot determine whether a given stream-state
  transition emits an event — treat as a gap and take 2B, and say so.
- Removing the interval would require adding a backend event to be safe: do NOT
  add it (out of scope); take 2B and file the missing-event as a follow-up.
- `npm test` reveals a test that depends on polling behavior.

## Maintenance notes

For the human/agent who owns this code after the change lands:

- If Step 2B was taken, the 15 s backstop is a stopgap. The proper fix is to emit
  `SnapshotChanged` from the backend on the uncovered transition(s) (likely the
  internal stream-error transition), then remove the backstop. The follow-up
  should live as its own plan touching `src-tauri/src`.
- This plan is compatible with Plan 009 and 010: those rely on the mutation
  `onSuccess` invalidations in `useStreams.ts`/`useConnection.ts`, which this
  plan explicitly does **not** touch.
- Reviewer should scrutinize: is there any passive/derived state (e.g. RTT-based
  "degraded" tray health) that used to refresh only via the 3 s snapshot poll?
  Stats arrive via `statsTick` (separate event, unaffected), so tray health keyed
  on stats is fine, but confirm nothing else silently depended on the interval.
