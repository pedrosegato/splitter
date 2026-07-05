# Plan 013: Persist trust/identity/settings files with 0600 permissions

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat 217a31d..HEAD -- crates/splitter-core/src/net/fs_util.rs crates/splitter-core/src/net/trust.rs crates/splitter-core/src/net/identity.rs crates/splitter-core/src/settings.rs`
> If any in-scope file changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Priority**: P1
- **Effort**: S
- **Risk**: LOW
- **Depends on**: none
- **Category**: security
- **Planned at**: commit `217a31d`, 2026-07-05

## Why this matters

The app persists bearer credentials — per-peer `auth_token` values in
`trusted_peers.toml` — as cleartext in the user's config directory. The write
path (`write_atomic`) creates those files with `std::fs::write`, so on Unix they
land at the process umask, typically `0644` (world-readable). Any other local
user account, or any process running as a different UID, can read every peer's
long-lived auth token straight off disk and then impersonate this node to its
trusted peers on the LAN. Persisting a secret world-readable is a credential
exposure regardless of whether it is ever exploited. After this plan, all config
files this app writes are created `0600` (owner read/write only) and the config
directory is `0700`, closing the local-disclosure vector.

## Current state

The facts the executor needs, inlined:

- `crates/splitter-core/src/net/fs_util.rs` — the single atomic-write helper used
  by trust/identity persistence. No permission is ever set. Full current body
  (lines 1–11):

  ```rust
  use std::path::Path;

  pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
      let tmp = path.with_extension(match path.extension() {
          Some(ext) => format!("{}.tmp", ext.to_string_lossy()),
          None => "tmp".into(),
      });
      std::fs::write(&tmp, bytes)?;
      std::fs::rename(&tmp, path)?;
      Ok(())
  }
  ```

  It already has a `#[cfg(test)] mod tests` block (lines 13–35) with
  `write_atomic_round_trips_content` and `write_atomic_leaves_no_tmp_file` —
  model the new test after those.

- `crates/splitter-core/src/net/trust.rs` — `TrustStore`. The credential type
  `TrustedPeer` carries `auth_token: String` (lines 9–14). `flush()` (lines
  146–155) serializes the store and calls `write_atomic(&self.path, ...)`. The
  config directory is created in `load_or_create` at lines 104–107 with
  `std::fs::create_dir_all(parent)` — no mode is set. `trust_store_path()`
  (lines 158–162) resolves `<config_dir>/Splitter/trusted_peers.toml`.

- `crates/splitter-core/src/net/identity.rs` — `PeerIdentity`. `save_atomic`
  (lines 14–23) and `load_or_create` (lines 25–52) both `create_dir_all(parent)`
  (no mode) and then call `write_atomic`. `identity_path()` (lines 55–59)
  resolves `<config_dir>/Splitter/identity.toml`.

- `crates/splitter-core/src/settings.rs` — `Settings::save_atomic` (lines
  126–144) does NOT use `write_atomic`; it has its OWN inlined tmp+rename using
  `std::fs::write` + `std::fs::rename` (lines 133–142) and `create_dir_all`
  (lines 127–130). This is a second copy of the same insecure pattern.

- Repo conventions: no code comments except a non-obvious WHY (project CLAUDE.md).
  Errors are the `NetError` enum, constructed as `NetError::ConfigIo(String)` at
  every I/O site here — match that. Tests live in an in-file
  `#[cfg(test)] mod tests` using `tempfile::tempdir()` (already a dev-dependency,
  used throughout these files).

There is currently **no** use of `set_permissions`, `PermissionsExt`, or `0o600`
anywhere in the crate. Confirm with the drift check + a grep (Done criteria).

## Commands you will need

| Purpose     | Command                                                        | Expected on success |
|-------------|---------------------------------------------------------------|---------------------|
| Build       | `cargo build --workspace`                                     | exit 0              |
| Tests       | `cargo test --workspace`                                      | all pass            |
| Focused test| `cargo test -p splitter-core fs_util`                         | new test passes     |
| Lint        | `cargo clippy --workspace --all-targets -- -D warnings`       | exit 0, no warnings |
| Format      | `cargo fmt --all -- --check`                                  | exit 0              |

## Scope

**In scope** (the only files you should modify):
- `crates/splitter-core/src/net/fs_util.rs` — make `write_atomic` create the temp
  file `0600` before rename; add the permission test.
- `crates/splitter-core/src/net/trust.rs` — set the config dir `0700` at its
  `create_dir_all` site (lines 104–107).
- `crates/splitter-core/src/net/identity.rs` — set the config dir `0700` at its
  two `create_dir_all` sites (lines 16–17 and 34–35).
- `crates/splitter-core/src/settings.rs` — replace the hand-rolled tmp+rename in
  `save_atomic` (lines 133–142) with a call to `write_atomic` so it inherits the
  `0600` behavior, and set the config dir `0700` at lines 127–130.

**Out of scope** (do NOT touch):
- The `SignalingMessage` / `HelloAck` wire format and the pairing/token-minting
  logic — that is plan 014.
- Any non-config file writes (audio buffers, logs).
- Windows ACL hardening — out of scope; the fix is best-effort `#[cfg(unix)]`
  and a no-op elsewhere (see Step 1).

## Git workflow

- Branch: `advisor/013-token-file-permissions`
- Commit style: conventional-commit **title only**, e.g.
  `fix(net): persist trust/identity/settings files as 0600`. No body. **Never**
  add a `Co-Authored-By` trailer.
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Create the atomic temp file with mode 0600 before rename

In `crates/splitter-core/src/net/fs_util.rs`, replace the `std::fs::write(&tmp, bytes)?;`
call so the temp file is created via `OpenOptions` with a Unix mode of `0o600`,
then rename as today. Because `rename` preserves the inode's permissions, the
final file inherits `0600`. Keep the function `pub(crate)` and the signature
unchanged.

Target shape:

```rust
use std::io::Write;
use std::path::Path;

pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension(match path.extension() {
        Some(ext) => format!("{}.tmp", ext.to_string_lossy()),
        None => "tmp".into(),
    });
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, path)?;
    Ok(())
}
```

Note: `.mode()` only affects a *newly created* file. If a stale `<name>.tmp`
from a prior crash already exists with looser perms, `OpenOptions` reuses it
without re-chmodding. Guard against that: after `open`, on `#[cfg(unix)]` call
`f.set_permissions(std::fs::Permissions::from_mode(0o600))?` (import
`std::os::unix::fs::PermissionsExt`) so the mode is enforced whether the file was
freshly created or reused. Keep this inside a single `#[cfg(unix)]` block.

**Verify**: `cargo build --workspace` → exit 0.

### Step 2: Add a Unix-only test asserting the written file mode is 0600

In the existing `#[cfg(test)] mod tests` in `fs_util.rs`, add a
`#[cfg(unix)]`-gated test that writes via `write_atomic` and asserts the file's
permission bits (`metadata.permissions().mode() & 0o777`) equal `0o600`. Model
it after `write_atomic_round_trips_content`.

Target shape:

```rust
#[cfg(unix)]
#[test]
fn write_atomic_sets_owner_only_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempdir().unwrap();
    let path = dir.path().join("secret.toml");
    write_atomic(&path, b"auth_token = \"x\"\n").expect("write");
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "config file must be owner-read/write only");
}
```

**Verify**: `cargo test -p splitter-core fs_util` → the new test passes.

### Step 3: Route settings.rs through write_atomic and drop the duplicate writer

In `crates/splitter-core/src/settings.rs`, in `save_atomic` (lines 126–144),
delete the hand-rolled `tmp` / `std::fs::write` / `std::fs::rename` block (lines
133–142) and replace it with a call to the shared helper:

```rust
use crate::net::fs_util::write_atomic;
// ...
write_atomic(path, raw.as_bytes())
    .map_err(|e| NetError::ConfigIo(format!("write {}: {e}", path.display())))?;
Ok(())
```

Add the `use` at the top of the file (identity.rs already imports it the same
way — mirror that). This removes the second insecure copy of the tmp+rename
pattern and gives settings the `0600` behavior for free.

**Verify**: `cargo test -p splitter-core settings` → all existing settings tests
still pass (they assert content round-trips and that no `.tmp` remains, which
`write_atomic` already satisfies).

### Step 4: Harden the config directory to 0700 at every create_dir_all site

At each of the four directory-creation sites, after `create_dir_all(parent)`
succeeds, set the directory mode to `0700` on Unix (best-effort — a failure to
chmod should surface as the same `NetError::ConfigIo`, not a panic). The sites:

- `trust.rs` lines 104–107 (`load_or_create`)
- `identity.rs` lines 16–17 (`save_atomic`) and 34–35 (`load_or_create`)
- `settings.rs` lines 127–130 (`save_atomic`)

To avoid four copies, add a small private helper in `fs_util.rs` and call it from
each site:

```rust
pub(crate) fn ensure_private_dir(parent: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
```

Then at each site replace the inline `create_dir_all(parent).map_err(...)` with
`crate::net::fs_util::ensure_private_dir(parent).map_err(|e| NetError::ConfigIo(format!("mkdir {}: {e}", parent.display())))?;`
(keep each site's existing error message wording).

**Verify**: `cargo build --workspace` → exit 0.

### Step 5: Full verification

Run the whole gate:

**Verify**:
- `cargo test --workspace` → all pass (including the new mode test).
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0.
- `cargo fmt --all -- --check` → exit 0.

## Test plan

- New test in `crates/splitter-core/src/net/fs_util.rs`:
  `write_atomic_sets_owner_only_permissions` (Unix-only) — asserts the written
  file is exactly `0o600`. This is the regression guard for the world-readable
  bug.
- Existing tests that must stay green (they exercise the changed paths):
  `settings::save_atomic_writes_via_tmp_then_rename`,
  `settings::save_then_load_round_trip_preserves_all_fields`,
  `identity::load_or_create_leaves_no_tmp_file`,
  `trust::add_leaves_no_tmp_file_and_content_round_trips`.
- Structural pattern to copy: the existing `mod tests` in `fs_util.rs`.
- Verification: `cargo test --workspace` → all pass, including 1 new test.

## Rotation requirement (MUST do / MUST document)

A code fix alone is insufficient: any `trusted_peers.toml` (and `identity.toml`)
that this app wrote **before** this change may already be sitting on disk
world-readable, and its tokens may already have been read by another local
account. Changing the writer does not re-chmod files that already exist and are
not rewritten.

This plan MUST therefore ship with an operator-facing rotation note (add it to
the PR description and to `## Maintenance notes` below), stating:

- Existing peer auth tokens should be considered potentially disclosed and
  **rotated** — i.e. re-pair the affected peers so a fresh token is minted
  (token minting itself is plan 014). Re-pairing replaces the stored token.
- As a defense-in-depth follow-up, on startup the app could chmod any existing
  `Splitter/*.toml` to `0600` and the dir to `0700` (a one-time migration). This
  is explicitly **deferred** out of this plan — call it out, do not implement it
  here unless the operator asks.

Do NOT print, log, or copy any token value while doing this. Reference files by
path only.

## Done criteria

Machine-checkable. ALL must hold:

- [ ] `cargo build --workspace` exits 0
- [ ] `cargo test --workspace` exits 0; `write_atomic_sets_owner_only_permissions`
      exists and passes
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` exits 0
- [ ] `cargo fmt --all -- --check` exits 0
- [ ] `grep -rn "std::fs::write" crates/splitter-core/src/settings.rs` returns no
      matches (the duplicate writer is gone)
- [ ] `grep -rn "0o600" crates/splitter-core/src/net/fs_util.rs` returns at least
      one match
- [ ] No files outside the in-scope list are modified (`git status`)
- [ ] `plans/README.md` status row updated
- [ ] PR description / maintenance notes contain the rotation requirement

## STOP conditions

Stop and report back (do not improvise) if:

- The code at the locations in "Current state" doesn't match the excerpts
  (the codebase drifted; e.g. `write_atomic` already sets a mode, or settings
  already routes through it).
- Any existing test that touches these files (`settings::*`, `identity::*`,
  `trust::*`) fails after the change and the failure is NOT about permissions —
  that means a behavioral regression the plan didn't intend.
- Achieving `0600` appears to require touching a file outside the in-scope list.
- The assumption "these config files are only ever written through
  `write_atomic` / `save_atomic`" turns out to be false (you find another writer
  of `trusted_peers.toml` / `identity.toml` / `settings.toml`).

## Maintenance notes

- If a new config file type is added, route its writes through `write_atomic` and
  its directory creation through `ensure_private_dir` — do not reintroduce a
  hand-rolled `std::fs::write` + `rename` (that is exactly the bug this plan
  removed from settings.rs).
- Reviewer should scrutinize: that the `#[cfg(unix)]` blocks compile to a no-op
  (not an error) on non-Unix, and that the stale-tmp `set_permissions` guard is
  present (a reused `.tmp` from a crash must still end up `0600`).
- **Rotation** (see the "Rotation requirement" section): existing tokens may be
  compromised; re-pairing to mint fresh tokens is the mitigation and depends on
  plan 014. A one-time startup chmod migration is deliberately deferred.
