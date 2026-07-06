# Plan 001: Gate the frontend and audit dependencies in CI

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat 217a31d..HEAD -- .github/workflows/ci.yml package.json`
> If either in-scope file changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Priority**: P1
- **Effort**: S
- **Risk**: LOW
- **Depends on**: none
- **Category**: dx
- **Planned at**: commit `217a31d`, 2026-07-05

## Why this matters

CI (`.github/workflows/ci.yml`) has four jobs — `fmt`, `clippy`, `test`,
`build` — and every one of them is Rust-only. The React/Vite/TypeScript
frontend under `src/` is never typechecked, tested, or built on push or PR.
The frontend only gets compiled at release time, because
`src-tauri/tauri.conf.json` wires `"beforeBuildCommand": "npm run build"`
(confirmed line 10) and Tauri's release build runs on a version tag. That means
a TypeScript type error or a failing `vitest` test can merge to `main`
completely unblocked and only surface when someone cuts a release. Adding a
`frontend` CI job closes that gap.

Separately, the app opens network listeners (TCP signaling, UDP audio, mDNS
discovery — see SPEC §5.4–5.6) and pulls a broad Rust dependency tree, yet
nothing checks those crates against the RustSec advisory database. A
`cargo-audit` job gives a standing vulnerability gate for a networked binary at
near-zero maintenance cost.

## Current state

- `.github/workflows/ci.yml` — the entire CI pipeline; four Rust jobs, no npm
  step anywhere. Triggers (lines 3–7):

  ```yaml
  on:
    push:
      branches: [ main ]
    pull_request:
    workflow_dispatch:

  env:
    CARGO_TERM_COLOR: always
  ```

  The simplest existing job, `fmt` (lines 13–20), shows the house style —
  `actions/checkout@v4`, `dtolnay/rust-toolchain@stable`, a single `run`:

  ```yaml
    fmt:
      runs-on: ubuntu-latest
      steps:
        - uses: actions/checkout@v4
        - uses: dtolnay/rust-toolchain@stable
          with:
            components: rustfmt
        - run: cargo fmt --all -- --check
  ```

  There is no `actions/setup-node` step and no `npm` string anywhere in the
  file. The matrix jobs (`clippy`, `test`, `build`) each install native audio
  deps per-OS, but the frontend jobs you add need none of that — they run on a
  plain `ubuntu-latest` with Node only.

- `package.json` (frontend, lines 5–11) — the three scripts this plan depends
  on already exist:

  ```json
    "scripts": {
      "dev": "vite",
      "build": "tsc --noEmit && vite build",
      "typecheck": "tsc --noEmit",
      "test": "vitest run"
    },
  ```

  Note `build` already runs `tsc --noEmit` before `vite build`, so `typecheck`
  is technically a subset of `build`. Run `typecheck` explicitly anyway — it
  fails faster and names the failing check clearly in the CI log.

- `src-tauri/tauri.conf.json` line 10 — `"beforeBuildCommand": "npm run build"`.
  This is the ONLY place the frontend is compiled in the current pipeline, and
  it fires on the Tauri release build, not on CI push/PR. (Out of scope to
  change — cited only to explain the gap.)

- There is a lockfile for reproducible installs. Confirm which one exists
  before writing the install step (see Step 1).

Repo convention: jobs use `actions/checkout@v4` and pin nothing else beyond
`@stable` / `@v4`. Match that — pin `actions/setup-node@v4`.

## Commands you will need

| Purpose            | Command                                              | Expected on success            |
|--------------------|------------------------------------------------------|--------------------------------|
| Which lockfile     | `ls package-lock.json npm-shrinkwrap.json 2>/dev/null` | prints at least one path       |
| YAML parses        | `python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/ci.yml'))"` | exit 0, no output              |
| npm steps present  | `grep -c 'npm ' .github/workflows/ci.yml`            | `>= 1`                         |
| Scripts exist      | `node -e "const s=require('./package.json').scripts; ['typecheck','test','build'].forEach(k=>{if(!s[k])throw new Error('missing '+k)})"` | exit 0, no output |
| cargo-audit job    | `grep -c 'cargo audit' .github/workflows/ci.yml`     | `>= 1`                         |

(The workflow itself runs on GitHub Actions and CANNOT be executed locally —
verification here is YAML validity plus the presence of the new job blocks.
See "Done criteria".)

## Suggested executor toolkit

- None required. This is a single YAML edit. Do NOT install `act` or attempt to
  run the workflow locally.

## Scope

**In scope** (the only file you should modify):
- `.github/workflows/ci.yml`

**Out of scope** (do NOT touch, even though they look related):
- `package.json` — you only READ it to confirm the three scripts exist; do not
  add or rename scripts.
- `src-tauri/tauri.conf.json` — the release-time build hook stays as is.
- `.github/workflows/release.yml` — the release pipeline is not part of this
  finding.
- Any `src/` source file — this plan does not fix frontend errors, it only
  makes CI surface them. If the new `frontend` job would fail on real errors,
  that is expected and belongs to a separate fix (see STOP conditions).

## Git workflow

- Branch: `advisor/001-frontend-ci-and-cargo-audit`
- One commit. Conventional-commit **title only**, no body. Example from repo
  history style: `refactor(types): newtype SessionId`. Use:
  `ci: add frontend typecheck/test/build and cargo-audit jobs`
- **NEVER** add a `Co-Authored-By:` trailer of any kind.
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Confirm the lockfile and the three npm scripts

Run `ls package-lock.json npm-shrinkwrap.json 2>/dev/null`. If
`package-lock.json` exists, the frontend job will use `npm ci` (requires a
lockfile) and `cache: npm` in `setup-node`. If NO lockfile exists, `npm ci`
will fail — in that case use `npm install` instead and OMIT the `cache: npm`
line (caching keys off the lockfile). Note which case you are in.

Then confirm the scripts:
`node -e "const s=require('./package.json').scripts; ['typecheck','test','build'].forEach(k=>{if(!s[k])throw new Error('missing '+k)})"`

**Verify**: the `node -e` command exits 0 with no output. If it throws
`missing <script>`, STOP — the plan's assumption is broken.

### Step 2: Add the `frontend` job to ci.yml

Append a new job to the `jobs:` map in `.github/workflows/ci.yml`. It runs on a
plain `ubuntu-latest` (no native audio deps needed — this is pure Node). Use
`npm ci` (or `npm install` per Step 1). Run the three scripts in one `run`
block so all three must pass:

```yaml
  frontend:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with:
          node-version: 20
          cache: npm
      - run: npm ci
      - run: npm run typecheck && npm test && npm run build
```

If Step 1 found no lockfile: replace `npm ci` with `npm install` and delete the
`cache: npm` line.

**Verify**: `python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/ci.yml'))"` → exit 0, and
`grep -c 'npm ' .github/workflows/ci.yml` → `>= 1`.

### Step 3: Add the `cargo-audit` job to ci.yml

Append a second new job. It installs `cargo-audit` and runs `cargo audit`.
`cargo audit` needs no native audio libs (it reads `Cargo.lock`, does not
compile the workspace), so `ubuntu-latest` with the Rust toolchain is enough.
Make it a **gate** (fail the job on any advisory) — this is the recommended
posture for a networked binary:

```yaml
  cargo-audit:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - name: Install cargo-audit
        run: cargo install cargo-audit --locked
      - run: cargo audit
```

If you later discover a pre-existing advisory that is not fixable within this
plan's scope (it would require a dependency bump), do NOT weaken the gate
silently — that is a STOP condition; report it so the operator can decide
between `cargo audit --ignore RUSTSEC-XXXX-YYYY` (documented) and a dep bump.

**Verify**: `python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/ci.yml'))"` → exit 0, and
`grep -c 'cargo audit' .github/workflows/ci.yml` → `>= 1`.

### Step 4: Final file-wide validity check

**Verify**:
- `python3 -c "import yaml,sys; d=yaml.safe_load(open('.github/workflows/ci.yml')); print(sorted(d['jobs']))"`
  → prints a list that INCLUDES `'frontend'` and `'cargo-audit'` alongside the
  original `'build'`, `'clippy'`, `'fmt'`, `'test'`.
- `git status --porcelain` → shows ONLY `.github/workflows/ci.yml` modified.

## Test plan

There are no unit tests for CI config. Verification is structural:

- The workflow YAML parses (`yaml.safe_load` exits 0).
- The `jobs` map contains six keys: `fmt`, `clippy`, `test`, `build`,
  `frontend`, `cargo-audit`.
- The `frontend` job invokes `npm run typecheck`, `npm test`, and
  `npm run build`.
- The `cargo-audit` job invokes `cargo audit`.
- No file outside scope changed.

Full end-to-end validation (the jobs actually passing) can only happen on
GitHub Actions after push, which is out of this plan's local scope. Note this
under "Bloqueios externos" in your final report if the operator expects a green
run: the run itself requires pushing, which this plan forbids without explicit
instruction.

## Done criteria

Machine-checkable. ALL must hold:

- [ ] `python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/ci.yml'))"` exits 0
- [ ] `python3 -c "import yaml; d=yaml.safe_load(open('.github/workflows/ci.yml')); assert 'frontend' in d['jobs'] and 'cargo-audit' in d['jobs']"` exits 0
- [ ] `grep -q 'npm run typecheck' .github/workflows/ci.yml && grep -q 'npm test' .github/workflows/ci.yml && grep -q 'npm run build' .github/workflows/ci.yml`
- [ ] `grep -q 'cargo audit' .github/workflows/ci.yml`
- [ ] `node -e "const s=require('./package.json').scripts; ['typecheck','test','build'].forEach(k=>{if(!s[k])throw new Error(k)})"` exits 0 (scripts still present, untouched)
- [ ] `git status --porcelain` lists only `.github/workflows/ci.yml`
- [ ] `plans/README.md` status row for 001 updated

## STOP conditions

Stop and report back (do not improvise) if:

- The `fmt` job block or the `on:`/`env:` header in `ci.yml` does not match the
  "Current state" excerpts — the workflow drifted since this plan was written.
- Step 1 reports a missing `typecheck`, `test`, or `build` script — the plan's
  core assumption is false.
- `cargo audit` (if you run it locally to sanity-check) reports an advisory you
  cannot resolve inside this plan's scope — report the RUSTSEC id (do NOT paste
  any secret or token) and let the operator choose ignore-vs-bump.
- Making the frontend or audit job pass would require editing any file outside
  the in-scope list (e.g. a real TS error in `src/`). This plan only adds the
  gate; fixing what the gate catches is separate work.

## Maintenance notes

For the human/agent who owns CI after this lands:

- If a Node version bump is needed later, change `node-version: 20` in the
  `frontend` job (and keep it aligned with whatever the release pipeline uses).
- The `cargo-audit` gate will start failing the day a new advisory lands
  against a pinned dependency — that is the intended behavior. Triage by
  bumping the crate; only use `cargo audit --ignore RUSTSEC-…` with a comment
  explaining WHY the advisory is not exploitable here.
- Reviewer should scrutinize: (1) `npm ci` vs `npm install` matches the
  lockfile reality, (2) the three scripts are chained with `&&` so a failure in
  any one fails the job, (3) no native audio deps were needlessly copied into
  these two Node/audit jobs.
- Deferred out of scope: wiring the frontend job into a required-status-check
  branch protection rule (a GitHub repo setting, not a file change).
