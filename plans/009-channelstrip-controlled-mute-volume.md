# Plan 009: Make ChannelStrip mute/volume reflect the server snapshot

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat 217a31d..HEAD -- src/features/routing/ChannelStrip.tsx src/features/routing/ChannelStrip.test.tsx src/features/routing/WireLayer.tsx`
> If any in-scope file changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Priority**: P1
- **Effort**: S
- **Risk**: LOW
- **Depends on**: none
- **Category**: bug
- **Planned at**: commit `217a31d`, 2026-07-05

## Why this matters

`ChannelStrip` holds mute and volume in **local component state** that is
initialized once and never re-synced with the authoritative server snapshot.
The M button reads a local `muted` boolean seeded to `false`; the volume
`Slider` is uncontrolled (`defaultValue`), so its initial position is frozen at
mount. Meanwhile `WireLayer` draws the same stream's mute/volume **directly
from the `stream` prop** (`stream.state === "paused" || stream.muted`, and
`stream.volume`). The two components therefore read from **different sources of
truth** for the same stream.

Concrete cost: after any change that arrives via the server snapshot — a remote
peer mutes the stream, the 3s snapshot poll, or a reconnect that re-fetches
state — the wire updates but the M button and slider do not. A muted wire can
sit next to a strip whose M button still shows unmuted, and vice versa. This is
a visible, reproducible contradiction in the UI. Deriving both values from the
`stream` prop makes the strip agree with the wire and with the backend.

## Current state

Files:

- `src/features/routing/ChannelStrip.tsx` — the channel strip (mute button +
  volume slider) for one stream. Holds the buggy local state.
- `src/features/routing/WireLayer.tsx` — draws the wires; already server-driven
  (the correct pattern to converge on).
- `src/bindings.ts` — generated Tauri types; defines `StreamSnapshot`.

`StreamSnapshot` carries the authoritative values (`src/bindings.ts`, the
`StreamSnapshot` type line):

```ts
export type StreamSnapshot = { id: StreamId; state: StreamState; source_peer: string; sink_peer: string; udp_port: number; source_device: string; sink_device: string; volume: number; muted: boolean }
```

`ChannelStrip.tsx` — the local-state bug (`src/features/routing/ChannelStrip.tsx:15-48`):

```tsx
export function ChannelStrip({ sessionId, stream, selected }: Props) {
  const selectStream = useUiStore((s) => s.selectStream);
  const streamControl = useStreamControl();
  const closeStream = useCloseStream();

  const [muted, setMuted] = useState(false);          // line 20 — ignores stream.muted

  const color = streamColor(stream.id);
  const initialVolume = Math.round(stream.volume * 100); // line 23 — read once, never updates

  const handleMute = useCallback(
    (e: React.MouseEvent) => {
      e.stopPropagation();
      const next = !muted;                              // toggles LOCAL value
      setMuted(next);
      streamControl.mutate({
        sessionId,
        streamId: stream.id,
        action: { type: "set_muted", muted: next },
      });
    },
    [muted, sessionId, stream.id, streamControl],
  );

  const handleVolumeChange = useCallback(
    (values: number[]) => {
      streamControl.mutate({
        sessionId,
        streamId: stream.id,
        action: { type: "set_volume", volume: values[0] / 100 },
      });
    },
    [sessionId, stream.id, streamControl],
  );
```

The M button reads `muted` (`ChannelStrip.tsx:98-102`) and the slider is
**uncontrolled** (`ChannelStrip.tsx:111-117`):

```tsx
<Slider
  defaultValue={[initialVolume]}
  min={0}
  max={100}
  onValueChange={handleVolumeChange}
  aria-label="volume"
/>
```

The correct, server-driven pattern is already in `WireLayer.tsx:56-62`:

```tsx
computed.push({
  id: stream.id,
  d: curve(a, b, centerX),
  color: streamColor(stream.id),
  muted: stream.state === "paused" || stream.muted,
  volume: stream.volume,
});
```

`Slider` (`src/components/ui/slider.tsx`) is a shadcn wrapper over Radix; it
already supports a controlled `value` prop (it prefers `value` over
`defaultValue` when both are arrays), so passing `value={[…]}` makes the thumb
track the prop.

### Design note the fix must honor

`WireLayer` treats a **paused** stream as visually muted
(`stream.state === "paused" || stream.muted`). The M button in the strip is
specifically the **mute** control and maps to the `set_muted` action, so its lit
state should reflect `stream.muted` (the mute flag the button owns), **not**
`state === "paused"`. Keep those semantics: derive the button's lit state from
`stream.muted` alone. Do not fold `state === "paused"` into the M button — that
would make the mute button light up for a non-mute reason and confuse the toggle
(clicking it would send `set_muted:false` on a stream that was only paused).

## Commands you will need

| Purpose   | Command                                             | Expected on success |
|-----------|-----------------------------------------------------|---------------------|
| Typecheck | `npm run typecheck`                                 | exit 0, no errors   |
| Tests     | `npm test -- src/features/routing/ChannelStrip.test.tsx` | all pass       |
| Full test | `npm test`                                          | all pass            |
| Build     | `npm run build`                                     | exit 0              |

## Suggested executor toolkit

- The repo has a `vercel-react-best-practices` skill available; consult it if
  you are unsure about controlled-vs-uncontrolled inputs, but the change here is
  small and this plan spells it out.

## Scope

**In scope** (the only files you should modify):
- `src/features/routing/ChannelStrip.tsx`
- `src/features/routing/ChannelStrip.test.tsx`

**Out of scope** (do NOT touch, even though they look related):
- `src/features/routing/WireLayer.tsx` — already correct; it is the reference,
  not a target.
- `src/hooks/useStreams.ts` — the `useStreamControl` mutation already invalidates
  `["snapshot"]` on success, so the refetch that carries the new value back is
  already wired. Do not change it.
- `src/bindings.ts` — generated file; never hand-edit.

## Git workflow

- Branch: `advisor/009-channelstrip-controlled-mute-volume`
- Commit style: conventional-commit **title only**, no body (repo convention —
  see `git log`, e.g. `refactor(types): newtype SessionId`). Example for this
  work: `fix(routing): derive ChannelStrip mute/volume from server snapshot`.
- **NEVER** add a `Co-Authored-By` trailer.
- Do NOT push or open a PR.

## Steps

### Step 1: Derive mute state from the `stream` prop

In `src/features/routing/ChannelStrip.tsx`:

- Delete `const [muted, setMuted] = useState(false);` (line 20) and the
  `useState` import if it becomes unused (check — `useCallback` stays).
- Introduce `const muted = stream.muted;` so the button reflects the server flag.
- In `handleMute`, compute `const next = !stream.muted;` (instead of `!muted`)
  and drop the `setMuted(next)` call. The mutation already fires; the new value
  returns via the `["snapshot"]` invalidation on mutation success. Update the
  `useCallback` dependency array: replace `muted` with `stream.muted`.

The M button JSX at lines 98-102 keeps reading `muted` — now a derived
`const` — so no change is needed there.

**Verify**: `npm run typecheck` → exit 0, no errors.

### Step 2: Make the volume Slider controlled

In the same file, replace the uncontrolled slider. Delete the
`const initialVolume = Math.round(stream.volume * 100);` line (23) and change
the `Slider` usage (lines 111-117) to controlled:

```tsx
<Slider
  value={[Math.round(stream.volume * 100)]}
  min={0}
  max={100}
  onValueChange={handleVolumeChange}
  aria-label="volume"
/>
```

`handleVolumeChange` stays as-is: it fires the `set_volume` mutation, whose
success invalidates `["snapshot"]`, and the refetched `stream.volume` drives the
thumb back. (Optional, note-only — do NOT implement unless a reviewer asks:
volume mutations fire on every drag tick; a future `useDebouncedSetter`-style
wrapper could batch them. Out of scope here.)

**Verify**: `npm run typecheck` → exit 0.

### Step 3: Extend the tests

See "Test plan". Then run the file and the full suite.

**Verify**: `npm test -- src/features/routing/ChannelStrip.test.tsx` → all pass,
then `npm test` → all pass.

## Test plan

Model new cases after the existing `src/features/routing/ChannelStrip.test.tsx`
(same `makeStream` factory, same `makeWrapper`, same mocks). The existing test
at lines 138-148 ("renders slider at correct initial volume") already asserts
`aria-valuenow === "70"` for `volume: 0.7` — keep it green.

Add two cases that prove the strip now follows the prop:

1. **Slider follows a re-rendered `stream` prop.** Render with
   `makeStream({ volume: 0.3 })`, assert the slider's `aria-valuenow` is `"30"`,
   then re-render the same component with `makeStream({ volume: 0.9 })` (use
   the `rerender` returned by `render`) and assert `aria-valuenow` is now `"90"`.
   This fails against the old `defaultValue` code and passes after Step 2.
2. **M button reflects `stream.muted` from the prop.** Render with
   `makeStream({ muted: true })`; assert the mute button is present with
   `aria-label="desmutar"` (the label is `muted ? "desmutar" : "mutar"`, line
   102). Re-render with `makeStream({ muted: false })` and assert the label is
   now `"mutar"`. This fails against the old `useState(false)` seed and passes
   after Step 1.

Verification: `npm test -- src/features/routing/ChannelStrip.test.tsx` → all
pass including the 2 new cases; `npm test` → all pass.

## Done criteria

Machine-checkable. ALL must hold:

- [ ] `npm run typecheck` exits 0
- [ ] `npm test` exits 0; the 2 new ChannelStrip cases exist and pass
- [ ] `grep -n "useState(false)" src/features/routing/ChannelStrip.tsx` returns no matches
- [ ] `grep -n "defaultValue" src/features/routing/ChannelStrip.tsx` returns no matches
- [ ] `npm run build` exits 0
- [ ] No files outside the in-scope list are modified (`git status`)
- [ ] `plans/README.md` status row for 009 updated

## STOP conditions

Stop and report back (do not improvise) if:

- The code at the locations in "Current state" doesn't match the excerpts
  (the codebase has drifted since this plan was written).
- The `Slider` component (`src/components/ui/slider.tsx`) does not accept a
  `value` prop or does not update the thumb when `value` changes — in that case
  the controlled approach needs a different component fix; report before
  editing the slider.
- A step's verification fails twice after a reasonable fix attempt.
- The fix appears to require touching `useStreams.ts`, `WireLayer.tsx`, or any
  out-of-scope file.

## Maintenance notes

For the human/agent who owns this code after the change lands:

- Reviewer should confirm the M button lit-state maps to `stream.muted` **only**
  (not `state === "paused"`) — see the "Design note" above — so it stays a true
  toggle for the `set_muted` action.
- The strip now depends on the `["snapshot"]` invalidation in `useStreamControl`
  to carry mutated values back. If that invalidation is ever removed (e.g. as
  part of Plan 011's polling changes), verify the strip still updates after a
  local mute/volume action. Plan 011 only touches poll intervals, not the
  mutation `onSuccess` invalidations, so the two are compatible.
- Deferred out of this plan: debouncing volume mutations during slider drag.
