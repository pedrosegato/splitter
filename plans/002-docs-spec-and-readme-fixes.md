# Plan 002: Correct the signaling spec and document build prerequisites

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat 217a31d..HEAD -- docs/SPEC.md README.md crates/splitter-core/src/net/signaling/message.rs`
> If any of these files changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition. Note: `message.rs` is the SOURCE OF
> TRUTH here and is NOT edited — you read it to correct the docs.

## Status

- **Priority**: P2
- **Effort**: S
- **Risk**: LOW
- **Depends on**: none
- **Category**: docs
- **Planned at**: commit `217a31d`, 2026-07-05

## Why this matters

Two docs have drifted from the code and will mislead anyone integrating with
the wire protocol or building the app from source.

(a) `docs/SPEC.md` §5.5 documents the signaling messages, but the
`StreamControl` shape it shows is wrong and the message list is incomplete. A
reader implementing the protocol from the spec would send malformed
`stream_control` frames and would not know that `set_muted`,
device-list, peer-rename, or stream-request messages exist. The real wire shape
lives in `crates/splitter-core/src/net/signaling/message.rs` and is enforced by
round-trip tests in that same file.

(b) `README.md`'s "Build" section is a bare `cargo build --workspace --release`
with no mention of the native libraries the build actually needs (opus
everywhere; webkit2gtk/gtk/appindicator/rsvg/alsa/udev on Linux; pkg-config).
Those prerequisites exist only inside the CI workflow, so a contributor on a
fresh machine hits an opaque linker/pkg-config failure. Documenting them turns a
frustrating dead end into a copy-paste setup step.

Both fixes are docs-only. Do NOT change `message.rs` or any workflow — the code
and CI are correct; the docs are what is stale.

## Current state

### (a) SPEC §5.5 vs message.rs

`docs/SPEC.md` §5.5 begins at line 225 (`### 5.5 Protocolo de signaling (TCP,
JSON)`) and documents messages as TypeScript-like types inside a ` ```ts ` block
running roughly lines 229–302.

The **incorrect** `StreamControl` type, `docs/SPEC.md:282-288`:

```ts
// Controle de stream em runtime
type StreamControl = {
  type: 'stream_control',
  stream_id: number,
  action: 'pause' | 'resume' | 'close' | 'set_volume',
  volume?: number,           // 0.0-1.0 se action=set_volume
}
```

The **incomplete** `HelloAck` type, `docs/SPEC.md:245-249`:

```ts
type HelloAck = {
  type: 'hello_ack',
  accepted: boolean,
  reason?: string,           // se rejected
}
```

The **real** shape, from `crates/splitter-core/src/net/signaling/message.rs`.
`StreamAction` is an internally-tagged enum (`#[serde(tag = "type",
rename_all = "snake_case")]`), so `action` is a NESTED object with its own
`type`, not a flat string — and it has FIVE variants including `set_muted`
(lines 49-58):

```rust
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamAction {
    Pause,
    Resume,
    Close,
    SetVolume { volume: f32 },
    SetMuted { muted: bool },
}
```

`StreamControl` embeds that action (lines 112-115):

```rust
    StreamControl {
        stream_id: u8,
        action: StreamAction,
    },
```

The round-trip test `stream_control_set_volume_carries_volume_in_variant`
(message.rs:281-291) proves the on-wire JSON is:

```json
{"type":"stream_control","stream_id":0,"action":{"type":"set_volume","volume":0.5}}
```

`HelloAck` has two extra optional fields the spec omits — `auth_token` and
`peer_id` (message.rs:81-89):

```rust
    HelloAck {
        accepted: bool,
        reason: Option<String>,
        auth_token: Option<String>,
        peer_id: Option<String>,
    },
```

Messages that exist in `message.rs` (the `SignalingMessage` enum, lines 70-133)
but are **entirely absent** from SPEC §5.5:

- `DeviceListRequest {}` (line 120) — empty body, tag `device_list_request`.
- `DeviceListResponse { devices: Vec<DeviceDescriptor> }` (lines 121-123), where
  `DeviceDescriptor` = `{ id: string, name: string, kind: DeviceKind }`
  (lines 10-14); `DeviceKind` serializes PascalCase: `Input` / `Output` /
  `SystemAudio` (test at message.rs:373-392).
- `PeerRenamed { peer_id: string, peer_name: string }` (lines 124-127).
- `StreamRequest { session_id, source: SourceKind, sink_device }` (lines
  128-132), where `SourceKind` is itself internally tagged
  (`#[serde(tag = "type", rename_all = "snake_case")]`, lines 16-22) with
  variants `mic { device_id }` and `system { device_id }`.

DO NOT reproduce any actual token value in the docs. `auth_token` is a
credential field — document it as a field of type `string` with a cross-ref to
§7; never paste a sample token.

### (b) README Build section vs CI native deps

`README.md:120-124` — the entire "Build" section:

```sh
## Build

​```sh
cargo build --workspace --release
​```
```

The native prerequisites are documented ONLY inside `.github/workflows/ci.yml`.
The Linux apt list (ci.yml:35, and repeated at lines 58 and 87):

```
libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libasound2-dev libudev-dev libopus-dev pkg-config
```

The macOS step (ci.yml:37-38): `brew install opus pkg-config`.
The Windows step (ci.yml:39-44): `vcpkg install opus:x64-windows`.

Existing README structure: `##` H2 section headers, fenced ` ```sh ` code
blocks (see the current "Build" and "Limitations" sections). Match that style —
add a `### ` subsection under "Build", or expand "Build" with per-OS blocks.

## Commands you will need

| Purpose                   | Command                                                              | Expected on success                    |
|---------------------------|---------------------------------------------------------------------|----------------------------------------|
| Locate spec section       | `grep -n '5.5 Protocolo de signaling' docs/SPEC.md`                 | prints line 225                        |
| Old StreamControl gone    | `grep -n "action: 'pause' \| 'resume' \| 'close' \| 'set_volume'" docs/SPEC.md` | no match (exit 1)          |
| set_muted documented      | `grep -c 'set_muted' docs/SPEC.md`                                  | `>= 1`                                 |
| Missing variants added    | `grep -c 'device_list_request\|peer_renamed\|stream_request' docs/SPEC.md` | `>= 3`                          |
| README deps documented    | `grep -c 'libwebkit2gtk-4.1-dev' README.md`                        | `>= 1`                                 |
| README opus/brew          | `grep -c 'brew install opus' README.md`                            | `>= 1`                                 |
| Source of truth unchanged | `git diff --stat crates/splitter-core/src/net/signaling/message.rs` | empty (no diff)                        |

## Scope

**In scope** (the only files you should modify):
- `docs/SPEC.md`
- `README.md`

**Out of scope** (do NOT touch):
- `crates/splitter-core/src/net/signaling/message.rs` — SOURCE OF TRUTH; read
  only. If the code and this plan disagree, the CODE wins and you STOP.
- `.github/workflows/ci.yml` and `release.yml` — you read the apt/brew/vcpkg
  lines to copy the package names; do not edit the workflows.
- Any other SPEC section outside §5.5.

## Git workflow

- Branch: `advisor/002-docs-spec-and-readme-fixes`
- Commit(s), conventional-commit **title only**, no body. Suggested:
  `docs(spec): correct §5.5 stream_control shape and list missing signaling messages`
  and `docs(readme): document native build prerequisites`
  (two commits, or one `docs: ...` if you prefer a single unit).
- **NEVER** add a `Co-Authored-By:` trailer of any kind.
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Fix `StreamControl` in SPEC §5.5

Replace the flat `StreamControl` type at `docs/SPEC.md:282-288` so `action` is
a nested internally-tagged object with all five variants. Keep the surrounding
`ts` code block and the Portuguese comment style consistent with the rest of
§5.5. Produce this shape (adjust prose to match the file's existing bilingual
tone):

```ts
// Controle de stream em runtime
type StreamControl = {
  type: 'stream_control',
  stream_id: number,
  action:
    | { type: 'pause' }
    | { type: 'resume' }
    | { type: 'close' }
    | { type: 'set_volume', volume: number }   // 0.0-1.0
    | { type: 'set_muted', muted: boolean },
}
```

**Verify**: `grep -n "action: 'pause' | 'resume' | 'close' | 'set_volume'" docs/SPEC.md`
→ no match (exit 1); and `grep -c "type: 'set_muted'" docs/SPEC.md` → `>= 1`.

### Step 2: Complete `HelloAck` in SPEC §5.5

Extend the `HelloAck` type at `docs/SPEC.md:245-249` to include the two optional
fields from message.rs:81-89. Do NOT paste any real token — `auth_token` is a
credential; describe it, cross-ref §7:

```ts
type HelloAck = {
  type: 'hello_ack',
  accepted: boolean,
  reason?: string,           // se rejected
  auth_token?: string,       // token persistido em TOFU, ver §7
  peer_id?: string,
}
```

**Verify**: `grep -c 'auth_token?: string' docs/SPEC.md` → `>= 1`.

### Step 3: Add the missing message types to SPEC §5.5

Inside the same §5.5 `ts` block (before the closing ` ``` ` at line 302), add
type definitions for the four omitted messages, matching message.rs exactly:

```ts
// Descoberta de dispositivos do peer remoto
type DeviceListRequest = {
  type: 'device_list_request',
}

type DeviceDescriptor = {
  id: string,
  name: string,
  kind: 'Input' | 'Output' | 'SystemAudio',
}

type DeviceListResponse = {
  type: 'device_list_response',
  devices: DeviceDescriptor[],
}

// Peer mudou de nome em runtime
type PeerRenamed = {
  type: 'peer_renamed',
  peer_id: string,
  peer_name: string,
}

// Pedido para o peer originar um stream a partir de uma fonte sua
type StreamRequest = {
  type: 'stream_request',
  session_id: string,
  source:
    | { type: 'mic', device_id: string }
    | { type: 'system', device_id: string },
  sink_device: string,
}
```

**Verify**: `grep -c 'device_list_request\|device_list_response\|peer_renamed\|stream_request' docs/SPEC.md`
→ `>= 4`.

### Step 4: Document native build prerequisites in README

Under the README "Build" section (currently `README.md:120-124`), add a
subsection listing the per-OS native dependencies, copied verbatim from the CI
workflow. Preserve the existing `cargo build --workspace --release` block; add
the prerequisites ABOVE it so a reader installs deps first. Target shape:

```md
## Build

### System dependencies

The workspace links against libopus (all platforms) plus, on Linux, the Tauri
GUI/audio stack. Install them before building:

**macOS**
​```sh
brew install opus pkg-config
​```

**Linux (Debian/Ubuntu)**
​```sh
sudo apt-get install -y libwebkit2gtk-4.1-dev libgtk-3-dev \
  libayatana-appindicator3-dev librsvg2-dev libasound2-dev \
  libudev-dev libopus-dev pkg-config
​```

**Windows** (via [vcpkg](https://vcpkg.io))
​```sh
vcpkg install opus:x64-windows
​```

### Compile

​```sh
cargo build --workspace --release
​```
```

Copy the Linux package list character-for-character from `.github/workflows/ci.yml:35`
so it never drifts from what CI actually installs.

**Verify**:
- `grep -c 'libwebkit2gtk-4.1-dev' README.md` → `>= 1`
- `grep -c 'brew install opus' README.md` → `>= 1`
- `grep -c 'vcpkg install opus' README.md` → `>= 1`

### Step 5: Confirm the source of truth was not touched

**Verify**:
- `git diff --stat crates/splitter-core/src/net/signaling/message.rs` → empty.
- `git status --porcelain` → lists ONLY `docs/SPEC.md` and `README.md`
  (plus `plans/README.md` when you update the index).

## Test plan

Docs-only change; no unit tests. Verification is grep-based (above) plus a
manual read-through: the corrected §5.5 must round-trip mentally against the
JSON in message.rs's tests — e.g. the `set_volume` example must read
`{"type":"stream_control","stream_id":0,"action":{"type":"set_volume","volume":0.5}}`,
matching `stream_control_set_volume_carries_volume_in_variant` (message.rs:281-291).
If the repo builds mdBook/docs anywhere, none is configured here, so there is no
docs build to run.

## Done criteria

Machine-checkable. ALL must hold:

- [ ] `grep -q "action: 'pause' | 'resume' | 'close' | 'set_volume'" docs/SPEC.md` returns NO match (old flat shape gone)
- [ ] `grep -q "set_muted" docs/SPEC.md`
- [ ] `grep -q "auth_token?: string" docs/SPEC.md`
- [ ] `grep -Eq 'device_list_request|device_list_response|peer_renamed|stream_request' docs/SPEC.md` (all four present)
- [ ] `grep -q 'libwebkit2gtk-4.1-dev' README.md && grep -q 'brew install opus' README.md && grep -q 'vcpkg install opus' README.md`
- [ ] `git diff --quiet crates/splitter-core/src/net/signaling/message.rs` (source of truth untouched)
- [ ] `git status --porcelain` lists only `docs/SPEC.md`, `README.md`, and `plans/README.md`
- [ ] No secret/token value appears in the diff (`git diff docs/SPEC.md README.md` shows field names/types only)
- [ ] `plans/README.md` status row for 002 updated

## STOP conditions

Stop and report back (do not improvise) if:

- Any excerpt in "Current state" does not match the live file (SPEC §5.5 or
  `message.rs` drifted) — the enum variants or field names differ from what is
  quoted here. The CODE wins; report the mismatch rather than guessing.
- The `message.rs` `StreamAction` / `SignalingMessage` enums have gained or lost
  variants relative to this plan — document what you actually find, and flag the
  delta so this plan can be refreshed.
- The CI apt list in `.github/workflows/ci.yml:35` differs from the package list
  quoted here — copy from the LIVE ci.yml, and note the difference.
- You find yourself needing to edit `message.rs`, any workflow, or a SPEC
  section other than §5.5 to make a verify pass — that is out of scope.

## Maintenance notes

For the human/agent who owns these docs after this lands:

- SPEC §5.5 and `message.rs` are now aligned but not mechanically linked. Any
  future change to the `SignalingMessage` or `StreamAction` enums must be
  mirrored into §5.5 by hand. A reviewer of a `message.rs` PR should check
  whether §5.5 needs the same edit.
- The README native-deps list is a copy of `ci.yml:35`. If CI's apt line
  changes (new system dep), update the README block to match — consider this the
  canonical follow-up trigger.
- Reviewer should scrutinize: (1) the nested `action` shape in §5.5 matches the
  `set_volume`/`set_muted` round-trip tests, (2) no literal token string leaked
  into the `auth_token` docs, (3) the Linux package list is verbatim from CI.
- Deferred out of scope: auto-generating the TS signaling types from Rust via
  the `specta` feature already present on several structs (`#[cfg_attr(feature =
  "specta", derive(specta::Type))]` in message.rs) — that would eliminate the
  manual sync entirely but is a separate build-tooling effort.
