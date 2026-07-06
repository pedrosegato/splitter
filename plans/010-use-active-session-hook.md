# Plan 010: Centralize "the active session" in a useActiveSession hook

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat 217a31d..HEAD -- src/features/routing/RoutingBoard.tsx src/features/routing/useWiring.ts src/features/stats/StatsView.tsx src/hooks`
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

"The active session" is recomputed independently in at least three components,
each with the same magic `find`:

- `src/features/routing/RoutingBoard.tsx:125` — `snapshots?.find((s) => s.state === "active") ?? null`
- `src/features/routing/useWiring.ts:19` — `(snap ?? []).find((s) => s.state === "active") ?? null`
- `src/features/stats/StatsView.tsx:97` — `sessions?.find((s) => s.state === "active")`

`RoutingBoard` and `useWiring` **each** call `useSnapshot()` **and**
`usePeerDevices(session?.remote_peer_id)` for the very same session — duplicated
fetch wiring for one logical thing. If the definition of "active" ever changes
(e.g. a session can be `active` but draining), every copy has to be found and
edited, and it is easy to miss one. A single `useActiveSession()` hook makes the
definition live in one place, removes the duplicated `usePeerDevices` call, and
gives the three call sites one consistent shape to consume.

React Query dedupes identical queries by key, so calling `useSnapshot()` in both
the hook and a component that also uses the hook does **not** add network
traffic. This plan reduces *code* duplication, not request count (that is Plan
011's concern).

## Current state

Files and their duplicated derivation:

- `src/features/routing/RoutingBoard.tsx` — top-level routing UI. Derives the
  session, streams, `remotePeerId`, and calls `usePeerDevices`
  (`RoutingBoard.tsx:113-155`):

```tsx
const { data: snapshots, isLoading: snapshotLoading } = useSnapshot();   // 115
...
const session = snapshots?.find((s) => s.state === "active") ?? null;    // 125
const connected = !!session;
const streams = useMemo(() => session?.streams ?? [], [session]);        // 127
...
const remotePeerId = session?.remote_peer_id;                            // 145
const remoteName = useRemoteName(remotePeerId);
const { data: peerDevices } = usePeerDevices(remotePeerId);              // 147
```

- `src/features/routing/useWiring.ts` — arming/click-to-wire logic. Independently
  derives the same session + peer devices (`useWiring.ts:12-20`):

```ts
const { data: snap } = useSnapshot();                                    // 12
const { data: devices } = useDevices();
const { data: identity } = useIdentity();
...
const session = (snap ?? []).find((s) => s.state === "active") ?? null;  // 19
const { data: peerDevices } = usePeerDevices(session?.remote_peer_id);   // 20
```

- `src/features/stats/StatsView.tsx` — stats panel. Derives the active session
  and flattens streams (`StatsView.tsx:95-103`):

```tsx
const { data: sessions } = useSnapshot();                                // 95
const activeSession = sessions?.find((s) => s.state === "active");       // 97
const activeStreamCount = activeSession?.streams.length ?? 0;
const allStreams = useMemo<StreamSnapshot[]>(
  () => sessions?.flatMap((s) => s.streams) ?? [],                       // 101
  [sessions],
);
```

Existing hook conventions to match (all in `src/hooks/`, thin wrappers over
`useQuery`):

- `src/hooks/useSnapshot.ts` — `export const useSnapshot = () => useQuery({ queryKey: ["snapshot"], ... })`
- `src/hooks/useDevices.ts` — exports `useDevices` and `usePeerDevices(peerId)`;
  `usePeerDevices` is `enabled: !!peerId`.

Existing hook test convention: `src/hooks/hooks.test.tsx` uses `renderHook` from
`@testing-library/react`, a `QueryClientProvider` wrapper (`makeWrapper`), and
mocks `@/lib/api`'s `commands` + `unwrap`. Model the new hook's test on it.

## Commands you will need

| Purpose   | Command                                          | Expected on success |
|-----------|--------------------------------------------------|---------------------|
| Typecheck | `npm run typecheck`                              | exit 0, no errors   |
| Tests     | `npm test -- <path to test file>`                | all pass            |
| Full test | `npm test`                                        | all pass            |
| Build     | `npm run build`                                  | exit 0              |

## Scope

**In scope** (the only files you should modify or create):
- `src/hooks/useActiveSession.ts` (create)
- `src/hooks/useActiveSession.test.tsx` (create)
- `src/features/routing/RoutingBoard.tsx`
- `src/features/routing/useWiring.ts`
- `src/features/stats/StatsView.tsx`

**Out of scope** (do NOT touch, even though they look related):
- `src/features/routing/useTrayHealth.ts` — it takes `snapshots` as a **prop**
  from `RoutingBoard` and does its own `flatMap(s => s.streams)` across **all**
  sessions (not just active) to derive tray health. Its semantics differ
  (all-sessions, not active-only); folding it into `useActiveSession` would
  change behavior. Leave it. `RoutingBoard` keeps passing `snapshots` to it.
- `src/hooks/useSnapshot.ts`, `src/hooks/useDevices.ts` — the new hook composes
  these; do not change them.
- `src/bindings.ts` — generated.

## Git workflow

- Branch: `advisor/010-use-active-session-hook`
- Commit style: conventional-commit **title only**, no body (see `git log`).
  Example: `refactor(routing): add useActiveSession hook`.
- **NEVER** add a `Co-Authored-By` trailer.
- Do NOT push or open a PR.

## Steps

### Step 1: Create the useActiveSession hook

Create `src/hooks/useActiveSession.ts`. It composes `useSnapshot` and
`usePeerDevices`, returning the derived session, its streams, the remote peer id,
and the remote peer devices. Match the existing thin-wrapper style; **no
comments** (repo rule).

```ts
import { useMemo } from "react";
import type { SessionSnapshot, StreamSnapshot, DeviceDescriptor } from "@/bindings";
import { useSnapshot } from "./useSnapshot";
import { usePeerDevices } from "./useDevices";

type ActiveSession = {
  session: SessionSnapshot | null;
  streams: StreamSnapshot[];
  remotePeerId: string | undefined;
  peerDevices: DeviceDescriptor[] | undefined;
  isLoading: boolean;
};

export function useActiveSession(): ActiveSession {
  const { data: snapshots, isLoading } = useSnapshot();
  const session = snapshots?.find((s) => s.state === "active") ?? null;
  const remotePeerId = session?.remote_peer_id;
  const { data: peerDevices } = usePeerDevices(remotePeerId);
  const streams = useMemo(() => session?.streams ?? [], [session]);
  return { session, streams, remotePeerId, peerDevices, isLoading };
}
```

Confirm the exact type names first: `grep -n "DeviceDescriptor\|SessionSnapshot\|StreamSnapshot" src/bindings.ts`. Use whatever the file actually exports.

**Verify**: `npm run typecheck` → exit 0.

### Step 2: Consume the hook in useWiring

In `src/features/routing/useWiring.ts`, replace the local `useSnapshot()` +
`find` + `usePeerDevices(session?.remote_peer_id)` (lines 12, 19, 20) with the
hook. Keep `useDevices()` and `useIdentity()` — those are separate. Result:

```ts
const { data: devices } = useDevices();
const { data: identity } = useIdentity();
const { session, peerDevices } = useActiveSession();
```

Remove the now-unused `useSnapshot` and `usePeerDevices` imports (keep
`useDevices`). The rest of the file (`onPortActivate`, the `session.id` /
`session.remote_peer_id` usage) is unchanged because `session` has the same
shape.

**Verify**: `npm run typecheck` → exit 0; `npm test -- src/features/routing/useWiring.test.tsx` → all pass.

### Step 3: Consume the hook in RoutingBoard

In `src/features/routing/RoutingBoard.tsx`, replace the derivations at lines
125, 127, 145, 147 with the hook, keeping every other local (identity, devices,
`snapshots` — see below):

```tsx
const { data: snapshots, isLoading: snapshotLoading } = useSnapshot();
const { session, streams, remotePeerId, peerDevices } = useActiveSession();
```

**Important**: `RoutingBoard` still needs the raw `snapshots` array to pass to
`useTrayHealth(snapshots)` (line 167). Keep the `useSnapshot()` call for that,
OR expose `snapshots` from the hook. The lower-friction option: keep the existing
`useSnapshot()` line for `snapshots`/`snapshotLoading`, and additionally call
`useActiveSession()` for `session`/`streams`/`remotePeerId`/`peerDevices`. React
Query dedupes the two `useSnapshot()` reads by key, so this is free. Then delete
the local `const session = … find …`, the `const streams = useMemo…`, the
`const remotePeerId = …`, and the `const { data: peerDevices } = usePeerDevices(remotePeerId)`
lines. `connected`, `remoteName`, `remoteSources/remoteSinks`, `wiredPortIds`,
`portColorMap` all keep referencing `session`/`streams`/`remotePeerId`/`peerDevices`
unchanged.

Remove the now-unused `usePeerDevices` import if nothing else uses it (check:
`useDevices` is still imported and used; only `usePeerDevices` may become unused).

**Verify**: `npm run typecheck` → exit 0; `npm test -- src/features/routing/RoutingBoard.test.tsx` → all pass.

### Step 4: Consume the hook in StatsView

In `src/features/stats/StatsView.tsx`, replace lines 95-103. `StatsView` needs
BOTH the active session (for `activeStreamCount`) AND **all** streams across all
sessions (`allStreams`, line 101, used to build `streamById`). `useActiveSession`
only gives active-session streams, so keep `useSnapshot()` for `allStreams`:

```tsx
const { data: sessions } = useSnapshot();
const { session: activeSession } = useActiveSession();
const activeStreamCount = activeSession?.streams.length ?? 0;
const allStreams = useMemo<StreamSnapshot[]>(
  () => sessions?.flatMap((s) => s.streams) ?? [],
  [sessions],
);
```

This removes the `find((s) => s.state === "active")` duplication (now inside the
hook) while preserving the all-sessions `flatMap`.

**Verify**: `npm run typecheck` → exit 0; `npm test -- src/features/stats/StatsView.test.tsx` → all pass.

### Step 5: Write the hook test and run the full suite

See "Test plan".

**Verify**: `npm test` → all pass; `npm run build` → exit 0.

## Test plan

Create `src/hooks/useActiveSession.test.tsx`, modeled structurally on
`src/hooks/hooks.test.tsx` (`renderHook`, `QueryClientProvider` wrapper via a
`makeWrapper`, mock `@/lib/api`'s `commands.snapshot` / `commands.peerDevices`
and `unwrap`). Cover:

1. **Picks the active session.** `commands.snapshot` resolves to a list with one
   `state: "active"` session (with `remote_peer_id` and a `streams` array) and
   one `state: "closed"` session. Assert `result.current.session?.state === "active"`,
   `result.current.remotePeerId` equals the active session's `remote_peer_id`,
   and `result.current.streams` equals the active session's streams.
2. **No active session → nulls.** `commands.snapshot` resolves to a list with
   only non-active sessions (or empty). Assert `session` is `null`, `streams` is
   `[]`, `remotePeerId` is `undefined`.
3. **peerDevices disabled when no remote.** With no active session, assert
   `commands.peerDevices` is not called (the `enabled: !!peerId` gate) — i.e.
   `expect(mockedCommands.peerDevices).not.toHaveBeenCalled()`.

Reuse the existing regression tests for the three consumers as-is
(`RoutingBoard.test.tsx`, `useWiring.test.tsx`, `StatsView.test.tsx`) — they must
stay green, proving the refactor is behavior-preserving.

Verification: `npm test` → all pass, including the 3 new `useActiveSession` cases.

## Done criteria

Machine-checkable. ALL must hold:

- [ ] `npm run typecheck` exits 0
- [ ] `npm test` exits 0; `src/hooks/useActiveSession.test.tsx` exists with 3 passing cases
- [ ] `grep -rn "state === \"active\"" src/features` returns no matches (all moved into the hook)
- [ ] `npm run build` exits 0
- [ ] No files outside the in-scope list are modified (`git status`)
- [ ] `plans/README.md` status row for 010 updated

## STOP conditions

Stop and report back (do not improvise) if:

- The code at the locations in "Current state" doesn't match the excerpts.
- Any of the three consumers' existing tests fail after the swap and the cause is
  a genuine behavior change (not a trivial mock adjustment) — that means the hook
  is not behavior-preserving; report before forcing it.
- You find a **fourth** site deriving the active session that this plan didn't
  list — note it and ask whether to fold it in, rather than silently expanding
  scope.
- The `useActiveSession` return shape can't satisfy all three consumers without
  changing their behavior.

## Maintenance notes

For the human/agent who owns this code after the change lands:

- If the definition of "active" gains nuance (e.g. `active` vs a new draining
  state), it now changes in exactly one place: `src/hooks/useActiveSession.ts`.
- `useTrayHealth` was deliberately left out because it operates over **all**
  sessions, not the active one. If tray health ever narrows to the active
  session, revisit.
- Reviewer should confirm `StatsView` still builds `streamById` from **all**
  streams (not just active) — the stats rows are keyed by `session_id-stream_id`
  and can reference non-active sessions.
