# Plan 014: Mint the pairing auth token server-side instead of trusting the client

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat 217a31d..HEAD -- crates/splitter-core/src/net/signaling/server.rs crates/splitter-core/src/net/signaling/client.rs crates/splitter-core/src/net/trust.rs`
> If any in-scope file changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Priority**: P1
- **Effort**: M
- **Risk**: MED
- **Depends on**: none (coordinates with plan 015 — see "Coordination" below)
- **Category**: security
- **Planned at**: commit `217a31d`, 2026-07-05

## Why this matters

The pairing credential is currently chosen by the *client* and is effectively
empty. On first contact the dialer sends `auth_token = ""` (an empty string),
and on accept the server stores exactly that client-proposed value as the
durable per-peer credential and echoes it back. Verification is a constant-time
compare, so an empty stored token accepts an empty presented token. Combined
with `auto_accept_trusted` defaulting to `true` and every node advertising its
`peer_id` publicly over mDNS, any device on the LAN can pair itself as a "trusted"
peer using a `peer_id` it read off the wire and an empty token — there is no
shared secret gating trust at all. Meanwhile the server already ships a proper
32-byte CSPRNG token generator (`generate_auth_token`) that has **zero call
sites** — the secure primitive exists and is simply never used. After this plan,
the server mints a fresh high-entropy token at accept time, stores and returns
*that*, ignores whatever the client proposed, and rejects tokens below an entropy
floor — turning the trust store into an actual bearer-secret store.

## Current state

The facts the executor needs, inlined:

- `crates/splitter-core/src/net/signaling/client.rs` — the dialer.
  - Lines 39–43: the token it presents is
    `trust.read().await.token_for(remote_peer_id_hint).unwrap_or_default()`.
    On first pairing there is no stored peer, so `token_for` returns `None` and
    `unwrap_or_default()` yields `""` (empty). That empty string is put into
    `SignalingMessage::Hello { auth_token: resolved_token, .. }` (line 51).
  - Lines 98–112: on `accepted`, the dialer persists the token it *received back*
    in `HelloAck.auth_token` into its own trust store via `t.add(TrustedPeer { ..
    auth_token: token, .. })`. So whatever the server returns is what the client
    remembers — meaning if the server returns a freshly minted token, the client
    already stores it correctly. **This half already works; verify, do not change.**

- `crates/splitter-core/src/net/signaling/server.rs` — the acceptor.
  - `PendingPeer` (lines 18–24) carries `proposed_token: String` — the raw
    client-sent value.
  - `generate_auth_token()` (lines 222–226): fills a `[0u8; 32]` from
    `rand::thread_rng()` and base64-encodes it. **This function has no callers.**
    Confirm with `grep -rn "generate_auth_token" crates/ src-tauri/` (only the
    definition should appear).
  - `accept_pending_as` (lines 238–278): takes the pending peer, then at line 252
    does `let token = p.proposed_token.clone();` and stores THAT as the durable
    credential via `t.add(TrustedPeer { .. auth_token: token.clone(), .. })`
    (lines 254–260) and returns it in `HelloAck.auth_token` (lines 262–270) and
    from the function `Ok((p.peer_id, token))` (line 277).
  - The inline auto-accept path for *already-trusted* peers (lines 182–196) echoes
    the peer's *existing* stored token (`auth_token.clone()` at line 189) — that
    is correct for re-connection and must be left alone; it only fires when
    `known && token_valid`.
  - The pre-accept Hello handler pushes `proposed_token: auth_token.clone()` into
    pending at lines 200–207.

- `crates/splitter-core/src/net/trust.rs` — the store.
  - `TrustedPeer { peer_id, peer_name, auth_token: String }` (lines 9–14).
  - `add` (lines 116–119) inserts and flushes — no validation of the token.
  - `verify` (lines 121–126) uses `constant_time_eq` on the stored vs presented
    token bytes; an empty stored token matches an empty presented token.

- `crates/splitter-core/src/net/signaling/message.rs` (lines 81–89): `HelloAck`
  already has `auth_token: Option<String>` and `peer_id: Option<String>`, both
  `#[serde(default, skip_serializing_if = "Option::is_none")]`. **The wire shape
  does not need to change** — you are only changing which value goes into
  `auth_token`. Do not alter this enum.

- Repo conventions: `NetError` for errors, no code comments except a non-obvious
  WHY, tests in an in-file `#[cfg(test)] mod tests` using `tempfile::tempdir()`.
  Both `client.rs` and `server.rs` already have extensive `#[tokio::test]` suites
  — model new tests after `server.rs::accept_pending_promotes_and_acks` and
  `client.rs::accept_pending_token_in_hello_ack_is_persisted_in_dialer_trust_store`.

## Coordination

Plan 015 also edits `crates/splitter-core/src/net/signaling/server.rs`, but a
different region: 015 touches the *pre-accept Hello loop* (lines ~106–210, adding
eviction of dropped pre-accept connections) and this plan touches
`accept_pending_as` (lines 238–278) plus the pending-push token value. The edits
do not overlap line-for-line, but if both land, whoever goes second must re-run
the drift check and re-read the other's region before editing. If 015 has already
landed when you start, confirm `accept_pending_as` still matches the excerpt
above before proceeding.

## Commands you will need

| Purpose      | Command                                                       | Expected on success |
|--------------|--------------------------------------------------------------|---------------------|
| Build        | `cargo build --workspace`                                    | exit 0              |
| Unit tests   | `cargo test -p splitter-core signaling`                      | all pass            |
| Trust tests  | `cargo test -p splitter-core trust`                          | all pass            |
| Integration  | `cargo test --workspace loopback`                            | passes (see STOP)   |
| Full tests   | `cargo test --workspace`                                     | all pass            |
| Lint         | `cargo clippy --workspace --all-targets -- -D warnings`      | exit 0, no warnings |
| Format       | `cargo fmt --all -- --check`                                 | exit 0              |

## Scope

**In scope** (the only files you should modify):
- `crates/splitter-core/src/net/signaling/server.rs` — mint the token in
  `accept_pending_as`; add tests.
- `crates/splitter-core/src/net/trust.rs` — add a token-length/entropy floor to
  `TrustStore::add`; add tests.

**Read-only verify (do NOT modify)**:
- `crates/splitter-core/src/net/signaling/client.rs` — confirm the dialer already
  persists the *returned* token (lines 98–112). No change expected; if a change
  seems required, that's a STOP condition.

**Out of scope** (do NOT touch):
- `SignalingMessage` / `HelloAck` wire format in `message.rs`.
- File permissions on the trust store — that is plan 013.
- The connection-eviction / pre-auth flooding fix — that is plan 015.
- The `auto_accept_trusted` default in `settings.rs` — changing it is a product
  decision, out of scope here (note it in maintenance notes instead).

## Git workflow

- Branch: `advisor/014-server-minted-auth-token`
- Commit style: conventional-commit **title only**, e.g.
  `fix(net): mint pairing auth token server-side`. No body. **Never** add a
  `Co-Authored-By` trailer.
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Add an entropy floor to TrustStore::add

In `crates/splitter-core/src/net/trust.rs`, change `add` (lines 116–119) to
reject a token that is too short to be a real secret. `generate_auth_token`
produces base64 of 32 random bytes (~43 chars), so a conservative floor of 32
characters rejects empty/trivial tokens while accepting every minted one.

Target shape:

```rust
pub const MIN_AUTH_TOKEN_LEN: usize = 32;

pub fn add(&mut self, peer: TrustedPeer) -> Result<(), NetError> {
    if peer.auth_token.len() < MIN_AUTH_TOKEN_LEN {
        return Err(NetError::SignalingProtocol {
            reason: "auth token below minimum entropy length".into(),
        });
    }
    self.trusted.insert(peer.peer_id, peer);
    self.flush()
}
```

Confirm `NetError::SignalingProtocol { reason: String }` is the right variant by
checking `crates/splitter-core/src/error.rs`; if the field name differs, match
the real one. (It is used with `reason:` throughout server.rs, so this shape is
expected.)

Note: several existing tests call `add` with short literal tokens like
`"tok-xyz"`, `"shared-tok"`, `"proposed"`. Those tests will now fail the floor.
You will update the in-crate ones you own in Step 4; the cross-crate callers are
covered by Step 3 (the production accept path stops using short tokens) and the
STOP condition below.

**Verify**: `cargo build --workspace` → exit 0 (compile only; tests come later).

### Step 2: Mint a fresh token in accept_pending_as; ignore the proposed value

In `crates/splitter-core/src/net/signaling/server.rs`, in `accept_pending_as`
(lines 238–278), replace line 252 `let token = p.proposed_token.clone();` with a
freshly minted token:

```rust
let token = generate_auth_token();
```

Everything downstream (the `t.add(TrustedPeer { .. auth_token: token.clone() })`,
the `HelloAck { auth_token: Some(token.clone()), .. }`, and the returned
`Ok((p.peer_id, token))`) then flows the minted value. The client-proposed
`p.proposed_token` is now unused in this function — leave the `PendingPeer` field
in place (it still carries the value the client sent, which the pre-accept path
populated), but do not read it here.

Because `t.add` now enforces the entropy floor (Step 1) and `generate_auth_token`
always exceeds it, the `t.add(...)?` at lines 254–260 will succeed for minted
tokens and its `?` will propagate a floor error if the primitive ever changed —
that is a correct invariant, not a bug.

**Verify**: `cargo build --workspace` → exit 0.

### Step 3: Confirm the auto-accept re-connect path is unaffected

Read lines 182–196 of `server.rs` (the `known && token_valid && auto_accept`
branch). Confirm it echoes the peer's *already-stored* token
(`auth_token.clone()`), NOT `generate_auth_token()`. This path is for a peer that
already paired and is reconnecting with a valid token — it must keep returning the
existing stored token so the client's stored token stays in sync. Make **no**
change here. If this path currently re-mints or rotates, STOP and report (the
plan assumed it does not).

**Verify**: (read-only) — no build needed; note the confirmation in your report.

### Step 4: Fix in-crate tests that assumed client-controlled / short tokens

Two suites now break because they assert the *proposed* token becomes the stored
token, or they seed short tokens directly via `add`:

- `server.rs::tests` — `accept_pending_promotes_and_acks` (lines 402–449) sends
  `auth_token: "proposed"` and reads back a token. It must no longer assert the
  returned token equals `"proposed"`. Instead assert the returned token is
  non-empty and `>= MIN_AUTH_TOKEN_LEN`, and that it differs from `"proposed"`.
- `client.rs::tests` — the tests that pre-seed both stores with a shared short
  token via `add` (`second_connect_with_stored_token_is_immediately_accepted`
  lines 285–345, `accept_does_not_overwrite_stored_peer_name_with_empty` lines
  411–480) use `"shared-tok"`, which is below the floor and now fails `add`.
  Replace those literals with a floor-satisfying constant, e.g.
  `let token = splitter_core::net::signaling::server::generate_auth_token();` (or
  a `"a".repeat(43)` literal) so `add` succeeds. Keep the tests' intent intact.
- `client.rs::tests` — the accept-path tests that go through `accept_pending` /
  `accept_pending_as` (e.g. `accept_pending_token_in_hello_ack_is_persisted_in_dialer_trust_store`
  lines 209–283, `dial_with_no_hint_accepted_persists_token_via_hello_ack_peer_id`
  lines 482–558) receive the *minted* token via the accept return value
  (`stored_token`) and assert the dialer's store verifies against it. These
  should keep passing unchanged because they already assert against the returned
  token, not a literal — confirm they pass; if one asserts the token equals an
  empty/proposed value, update it to assert against the returned `stored_token`.

**Verify**: `cargo test -p splitter-core signaling` and
`cargo test -p splitter-core trust` → all pass.

### Step 5: Add the new security regression tests

Add these tests (see Test plan for exact assertions):

1. In `server.rs::tests`: `accept_mints_non_empty_high_entropy_token` — client
   sends an empty `auth_token`; after `accept_pending`, the returned token is
   non-empty, `>= MIN_AUTH_TOKEN_LEN`, and NOT equal to `""`.
2. In `server.rs::tests`: `empty_proposed_token_is_not_the_stored_credential` —
   after accept with an empty proposed token, `trust.verify(peer_id, "")` is
   `false` and `trust.verify(peer_id, &returned_token)` is `true`.
3. In `trust.rs::tests`: `add_rejects_below_min_length` — `add` with a 5-char
   token returns `Err`; with a 43-char token returns `Ok`.
4. In `trust.rs::tests`: `verify_rejects_wrong_token` already exists in spirit
   (`add_persists_and_verify_succeeds_on_reload` asserts `!verify(.., "wrong")`)
   — ensure an explicit case remains after your `add` edits (its token must now
   satisfy the floor).

**Verify**: `cargo test -p splitter-core` → all pass, including the 3 new tests.

### Step 6: Run the integration test and the full gate

**Verify**:
- `cargo test --workspace loopback` → the `loopback_two_daemons` integration test
  passes. **If it fails in an unexpected way, see STOP conditions — do not
  paper over it.**
- `cargo test --workspace` → all pass.
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0.
- `cargo fmt --all -- --check` → exit 0.

## Test plan

New / updated tests:
- `server.rs`: `accept_mints_non_empty_high_entropy_token` (happy path: minted
  token is real), `empty_proposed_token_is_not_the_stored_credential` (the exact
  bug this plan fixes: empty client token must not become the credential).
- `trust.rs`: `add_rejects_below_min_length` (floor enforced),
  wrong-token rejection preserved.
- Updated: `server.rs::accept_pending_promotes_and_acks` and the `client.rs`
  seed-with-shared-token tests, per Step 4.
- Structural pattern: `server.rs::tests::accept_pending_promotes_and_acks` for
  the accept-flow tests; `trust.rs::tests::add_persists_and_verify_succeeds_on_reload`
  for the trust tests.
- Verification: `cargo test --workspace` → all pass.

## Rotation requirement (MUST do / MUST document)

This is a credential-model change, so a code fix alone is not enough:

- **Both ends must upgrade.** An old client that still sends an empty token will,
  after this change, be minted a fresh token by the server and will store the
  returned token correctly (client.rs lines 98–112 already do this) — so a new
  server + old client still pairs securely. But an old *server* paired with a new
  client will still store the empty token. Ship this as a coordinated upgrade and
  say so in the PR.
- **Existing empty-token pairings are compromised and must be rotated.** Any
  `TrustedPeer` currently persisted with an empty (or trivially short) token was
  never a real secret. Those peers must **re-pair** so a fresh token is minted;
  document this in the PR and in `## Maintenance notes`. Note that plan 013 hardens
  the file permissions but does not rotate values — rotation here means re-pairing.
- Do NOT print or log any token value during testing or rollout. Reference peers
  by `peer_id` only.

## Done criteria

Machine-checkable. ALL must hold:

- [ ] `cargo build --workspace` exits 0
- [ ] `cargo test --workspace` exits 0; the 3 new tests exist and pass
- [ ] `cargo test --workspace loopback` passes
- [ ] `grep -rn "generate_auth_token" crates/splitter-core/src/net/signaling/server.rs`
      shows the definition AND at least one call inside `accept_pending_as`
- [ ] `grep -n "proposed_token.clone()" crates/splitter-core/src/net/signaling/server.rs`
      returns no match inside `accept_pending_as` (the client value is no longer
      the stored credential)
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` exits 0
- [ ] `cargo fmt --all -- --check` exits 0
- [ ] `crates/splitter-core/src/net/signaling/client.rs` is unmodified
      (`git status` — read-only verify only)
- [ ] No files outside the in-scope list are modified (`git status`)
- [ ] `plans/README.md` status row updated
- [ ] PR description / maintenance notes contain the rotation requirement

## STOP conditions

Stop and report back (do not improvise) if:

- The code at the locations in "Current state" doesn't match the excerpts —
  especially if `generate_auth_token` already has a call site, or
  `accept_pending_as` no longer reads `proposed_token`.
- Making the dialer persist the returned token appears to require editing
  `client.rs` — the plan asserts client.rs already does this (lines 98–112); if
  it doesn't, the trust model differs from what this plan assumed.
- Changing the token flow breaks `loopback_two_daemons` (or any other
  integration test) in a way you can't attribute to a short-token literal that
  Step 4 should have fixed — report the exact failing assertion; do not weaken
  the entropy floor to make it pass.
- The entropy floor rejects a token the production accept path legitimately
  produces (it should not — `generate_auth_token` is ~43 chars).

## Maintenance notes

- The security of pairing now rests on: (a) the server minting the token, and
  (b) `auto_accept_trusted`. Even with server-minted tokens, `auto_accept_trusted
  = true` (default in settings.rs line 60) means a *first* pairing is accepted
  without user confirmation. Whether first-contact should require an explicit
  accept is a product decision deliberately left out of this plan — flag it for
  the owner.
- If the token format ever changes (length/encoding), revisit `MIN_AUTH_TOKEN_LEN`
  so the floor still admits every minted token.
- Reviewer should scrutinize: that the auto-accept *re-connect* path (lines
  182–196) still echoes the existing stored token and does NOT re-mint (re-minting
  there would desync the client's stored token on every reconnect).
