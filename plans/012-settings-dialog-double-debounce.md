# Plan 012: Fix double-debounced number inputs in SettingsDialog

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat 217a31d..HEAD -- src/features/settings/SettingsDialog.tsx src/features/settings/SettingsDialog.test.tsx src/features/settings/useSettingsForm.ts`
> If any in-scope file changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Priority**: P2
- **Effort**: S
- **Risk**: LOW
- **Depends on**: none
- **Category**: bug
- **Planned at**: commit `217a31d`, 2026-07-05

## Why this matters

`SettingsDialog` debounces number inputs **twice** for some fields and **once**
for another — inconsistent, and slower than intended.

The `NumberInput` component **already debounces internally**: it wraps the `set`
it receives with `useDebouncedSetter(set)` (300 ms) and calls the debounced
version on change (`SettingsDialog.tsx:150,166`). But two of its call sites pass
`set={debouncedSet}` — where `debouncedSet` is a *second* `useDebouncedSetter`
created in the dialog body (`SettingsDialog.tsx:221`). So those fields get
300 ms + 300 ms ≈ **600 ms** of latency before a value is saved:

- signaling port (`SettingsDialog.tsx:286`, `set={debouncedSet}`)
- jitter max depth (`SettingsDialog.tsx:359`, `set={debouncedSet}`)

Meanwhile the fixed-jitter input passes a **non-debounced** inline setter
(`SettingsDialog.tsx:346`, `set={(_, v) => set("jitter_mode", \`fixed:${v}\`)}`),
so it debounces only once (~300 ms). Three number inputs, two different latencies,
none of them the intended single 300 ms for all. The fix is to always pass the
**raw** `set` to `NumberInput` (it debounces internally) and delete the redundant
`debouncedSet`.

## Current state

`src/features/settings/SettingsDialog.tsx`.

The internal debounce inside `NumberInput` (`SettingsDialog.tsx:134-171`,
key lines):

```tsx
function NumberInput({ id, settingKey, value, min, max, set }: {...}) {
  const [local, setLocal] = useState(String(value));
  const debouncedSet = useDebouncedSetter(set);      // 150 — INTERNAL debounce

  useEffect(() => { setLocal(String(value)); }, [value]);

  return (
    <Input
      ...
      onChange={(e) => {
        setLocal(e.target.value);
        const n = Number(e.target.value);
        if (!Number.isNaN(n)) debouncedSet(settingKey, n);   // 166 — calls the internal debounce
      }}
      ...
    />
  );
}
```

The redundant second debounce in the dialog body (`SettingsDialog.tsx:221`):

```tsx
const debouncedSet = useDebouncedSetter(set);        // 221 — the redundant one
```

The three `NumberInput` call sites:

```tsx
// signaling port — SettingsDialog.tsx:280-287 → double-debounced
<NumberInput id="signaling-port" settingKey="signaling_port"
  value={settings.signaling_port} min={1024} max={65535} set={debouncedSet} />

// jitter fixed ms — SettingsDialog.tsx:340-347 → single-debounced, inline setter
<NumberInput id="jitter-fixed-ms" settingKey="jitter_mode"
  value={jitter.fixedMs} min={0} max={500}
  set={(_, v) => set("jitter_mode", `fixed:${v}`)} />

// jitter max depth — SettingsDialog.tsx:353-360 → double-debounced
<NumberInput id="jitter-max-depth" settingKey="jitter_max_depth_ms"
  value={settings.jitter_max_depth_ms} min={0} max={1000} set={debouncedSet} />
```

`useDebouncedSetter` (`SettingsDialog.tsx:62-71`) — a `useCallback` that wraps a
`setTimeout(set, 300)`. `set` itself comes from `useSettingsForm()`
(`src/features/settings/useSettingsForm.ts:20-30`): it maps the value to a
string and calls the `useSetSetting` mutation immediately (no debounce of its
own). So `set` is the raw, un-debounced setter — safe to pass straight to
`NumberInput`, which debounces it once internally.

**Confirmed**: `debouncedSet` (the one at line 221) is used ONLY by the two
`NumberInput` call sites (280-287, 353-360). It is not used by any switch,
select, or other field (those call `set(...)` directly — e.g. lines 274, 296,
309). So it becomes dead once those two call sites switch to `set`.

## Commands you will need

| Purpose   | Command                                                       | Expected on success |
|-----------|---------------------------------------------------------------|---------------------|
| Typecheck | `npm run typecheck`                                          | exit 0, no errors   |
| Tests     | `npm test -- src/features/settings/SettingsDialog.test.tsx`  | all pass            |
| Full test | `npm test`                                                   | all pass            |
| Build     | `npm run build`                                              | exit 0              |

## Scope

**In scope** (the only files you should modify):
- `src/features/settings/SettingsDialog.tsx`
- `src/features/settings/SettingsDialog.test.tsx`

**Out of scope** (do NOT touch, even though they look related):
- `src/features/settings/useSettingsForm.ts` — read it to confirm `set` is the
  raw setter (it is; the mutation fires immediately). Do NOT add debouncing here;
  the single debounce belongs inside `NumberInput`.
- `src/features/settings/useSettingsForm.test.ts` — leave green as-is.
- The `useDebouncedSetter` helper definition itself (lines 62-71) — it stays; it
  is still used inside `NumberInput`. You are only removing the redundant
  *instance* at line 221, not the function.

## Git workflow

- Branch: `advisor/012-settings-dialog-double-debounce`
- Commit style: conventional-commit **title only**, no body (see `git log`).
  Example: `fix(settings): remove double debounce on number inputs`.
- **NEVER** add a `Co-Authored-By` trailer.
- Do NOT push or open a PR.

## Steps

### Step 1: Pass the raw `set` to the two double-debounced inputs

In `src/features/settings/SettingsDialog.tsx`:

- Signaling port (line 286): change `set={debouncedSet}` → `set={set}`.
- Jitter max depth (line 359): change `set={debouncedSet}` → `set={set}`.

Leave the fixed-jitter input (line 346) as-is for now — it needs the inline
`fixed:${v}` transform (its `settingKey` is `jitter_mode` but the value must be
serialized as `fixed:<n>`). It is already single-debounced via `NumberInput`'s
internal `useDebouncedSetter`, which is the target latency. (Do not "simplify" it
to `set={set}` — that would send a bare number to `jitter_mode`, which is wrong.)

**Verify**: `npm run typecheck` → exit 0.

### Step 2: Delete the redundant `debouncedSet`

Remove line 221 `const debouncedSet = useDebouncedSetter(set);`. After Step 1 it
has no remaining references. Confirm with
`grep -n "debouncedSet" src/features/settings/SettingsDialog.tsx` → the only
matches should be **inside** `NumberInput` (lines ~150 and ~166), NOT in the
dialog body.

`useDebouncedSetter` (the function) stays — it is still used inside
`NumberInput`. Do not remove the import/definition.

**Verify**: `npm run typecheck` → exit 0 (no unused-variable error for
`debouncedSet`).

### Step 3: Update/extend the tests

See "Test plan".

**Verify**: `npm test -- src/features/settings/SettingsDialog.test.tsx` → all
pass; then `npm test` → all pass.

## Test plan

Model after the existing `src/features/settings/SettingsDialog.test.tsx`, which
mocks `useSettingsForm` to expose a `mockSet` spy and renders the dialog. The
existing test does not currently exercise the number inputs, so add a case that
pins the single-debounce behavior.

Use fake timers to assert exactly one debounce (300 ms), not 600 ms:

1. **Signaling port debounces once (300 ms).** With `vi.useFakeTimers()`:
   render the dialog, find the signaling-port input
   (`within(document.body).getByLabelText("Porta de sinalização")` — the
   `SettingLabel htmlFor="signaling-port"` links it; if the label association
   doesn't resolve, select by `document.body.querySelector('[id="signaling-port"]')`).
   Fire a `change` to a valid value (e.g. `"7400"`). Assert `mockSet` has NOT
   been called yet. Advance timers by 300 ms (`vi.advanceTimersByTime(300)` inside
   `act`). Assert `mockSet` was called once with `("signaling_port", 7400)`.
   Restore real timers in a `finally`/`afterEach` (`vi.useRealTimers()`).

   Before the fix this test FAILS (at 300 ms the value is still stuck in the
   outer debounce; it would need 600 ms), proving the double-debounce bug; after
   the fix it passes.

2. **(Optional, if timer wiring is fiddly) jitter max depth debounces once.**
   Same pattern for the `jitter-max-depth` input asserting
   `("jitter_max_depth_ms", <n>)` fires at 300 ms.

Keep every existing SettingsDialog test green (switches, selects, theme buttons,
device name, reset).

Verification: `npm test -- src/features/settings/SettingsDialog.test.tsx` → all
pass including the new debounce case; `npm test` → all pass.

## Done criteria

Machine-checkable. ALL must hold:

- [ ] `npm run typecheck` exits 0 (no unused `debouncedSet`)
- [ ] `npm test` exits 0; a new test asserting single 300 ms debounce for a
      number input exists and passes
- [ ] `grep -n "debouncedSet" src/features/settings/SettingsDialog.tsx` shows
      matches ONLY inside `NumberInput` (none in the dialog body / call sites)
- [ ] `grep -n "set={debouncedSet}" src/features/settings/SettingsDialog.tsx`
      returns no matches
- [ ] `npm run build` exits 0
- [ ] No files outside the in-scope list are modified (`git status`)
- [ ] `plans/README.md` status row for 012 updated

## STOP conditions

Stop and report back (do not improvise) if:

- The code at the locations in "Current state" doesn't match the excerpts.
- After Step 1, `grep` shows `debouncedSet` is still referenced somewhere other
  than the two call sites you changed — that means a third consumer exists this
  plan didn't account for; report before deleting line 221.
- The fixed-jitter input (line 346) turns out NOT to need the `fixed:${v}`
  transform (i.e. the excerpt drifted) — reassess before changing it.
- A test's fake-timer assertion can't distinguish 300 ms from 600 ms after two
  attempts — report the timing setup rather than loosening the assertion into
  something that would pass for both.

## Maintenance notes

For the human/agent who owns this code after the change lands:

- The single source of debounce for number inputs is now `NumberInput`'s internal
  `useDebouncedSetter(set)` (line 150). Any new number field should pass the raw
  `set` (or a raw inline transform like the fixed-jitter one) — never a
  pre-debounced setter.
- Reviewer should confirm the fixed-jitter input still serializes to
  `fixed:<n>` and was intentionally left with its inline transform (single
  debounce), not accidentally normalized to `set={set}`.
- If a future field needs a *different* debounce delay, pass it via a prop to
  `NumberInput` rather than re-introducing an outer `useDebouncedSetter`.
