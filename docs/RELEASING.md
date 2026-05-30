# Releasing Splitter

This document describes how to cut a production release. The CI workflow
(`release.yml`) handles building for all platforms, signing updater artifacts,
and creating a GitHub Release draft. You review and publish the draft manually.

---

## Version bump

Three files must stay in sync. Update them all in a single commit before
tagging:

| File | Field |
|------|-------|
| `Cargo.toml` (workspace root) | `[workspace.package] version` |
| `src-tauri/tauri.conf.json` | top-level `"version"` |
| `package.json` | `"version"` |

```sh
# Example: bumping to 0.2.0
# Edit the three files above, then:
git add Cargo.toml src-tauri/tauri.conf.json package.json
git commit -m "chore(release): bump version to 0.2.0"
```

---

## Tag and push

```sh
git tag v0.2.0
git push origin v0.2.0
```

Pushing the tag triggers `.github/workflows/release.yml`. The workflow:

1. Builds the Tauri app on macOS (Apple Silicon + Intel), Windows, and Linux.
2. Signs the updater artifacts (`latest.json` + bundles) with the
   `TAURI_SIGNING_PRIVATE_KEY` secret.
3. Creates or updates a **draft** GitHub Release named `Splitter v0.2.0` and
   uploads all bundles (`*.dmg`, `*.msi`, `*.exe`, `*.AppImage`, `*.deb`) plus
   `latest.json`.

You can also trigger the workflow manually from the Actions tab
(`workflow_dispatch`) without pushing a tag — useful for testing.

---

## Review and publish the draft

1. Open [Releases](https://github.com/pedrosegato/splitter/releases) and find
   the draft.
2. Verify the expected assets are attached (two `.dmg` files, `*.msi`,
   `*.exe`, `*.AppImage`, `*.deb`, `latest.json`).
3. Edit the release notes if needed, then click **Publish release**.

Publishing makes `latest.json` the active update target — running desktop
clients will pick it up on their next update check.

---

## Required GitHub Actions secrets

### Always required

| Secret | Value |
|--------|-------|
| `TAURI_SIGNING_PRIVATE_KEY` | Full contents of `~/.tauri/splitter.key` (the minisign private key generated during project setup) |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Password for the key — leave blank if the key has no password |

These are used to sign the updater artifacts. Without them the build still
succeeds but the updater cannot verify downloaded bundles.

### Deferred: macOS code-signing and notarization

These secrets are **not yet provisioned** (planned for a later release phase).
Without them the `.app` and `.dmg` are unsigned — users see a Gatekeeper
warning on first launch (right-click → Open, or `xattr -dr com.apple.quarantine`).

| Secret | Description |
|--------|-------------|
| `APPLE_CERTIFICATE` | Base64-encoded `.p12` Developer ID certificate |
| `APPLE_CERTIFICATE_PASSWORD` | Password for the `.p12` |
| `APPLE_SIGNING_IDENTITY` | Identity string, e.g. `Developer ID Application: Acme Corp (TEAMID)` |
| `APPLE_ID` | Apple ID used for notarization (e.g. `dev@example.com`) |
| `APPLE_PASSWORD` | App-specific password for that Apple ID |
| `APPLE_TEAM_ID` | 10-character Apple Team ID |

When these are provisioned, uncomment the corresponding `env:` lines in
`.github/workflows/release.yml` (lines marked `# APPLE_*`).

### Deferred: Windows Authenticode signing

These secrets are **not yet provisioned** (planned for a later release phase).
Without them the installer triggers a SmartScreen warning (More info → Run anyway).

| Secret | Description |
|--------|-------------|
| `WINDOWS_CERTIFICATE` | Base64-encoded `.pfx` Authenticode certificate |
| `WINDOWS_CERTIFICATE_PASSWORD` | Password for the `.pfx` |

When these are provisioned, uncomment the corresponding `env:` lines in
`.github/workflows/release.yml` (lines marked `# WINDOWS_*`).

---

## Landing page

The landing page (`landing/`) is deployed to GitHub Pages automatically by
`.github/workflows/pages.yml` on every push to `release/mvp` or `main`.
No manual step is needed.
