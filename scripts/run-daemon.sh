#!/usr/bin/env bash
set -euo pipefail

PEER_NAME="${1:-alice}"
PORT="${2:-7001}"
IDENTITY_DIR="$HOME/.audiomirror/$PEER_NAME"

export PATH="$HOME/.cargo/bin:$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$PATH"
export CMAKE_POLICY_VERSION_MINIMUM=3.5

echo "=== AudioMirror daemon launcher ==="
echo "peer_name: $PEER_NAME"
echo "port:      $PORT"
echo "identity:  $IDENTITY_DIR"
echo

mkdir -p "$IDENTITY_DIR"

echo "[1/3] git pull..."
git pull

echo
echo "[2/3] cargo build --release..."
cargo build -p audiomirror-cli --release

echo
echo "=== Local audio devices ==="
./target/release/audiomirror-cli devices
echo

echo "[3/3] launching daemon..."
echo "press Ctrl+C inside daemon for graceful shutdown."
echo
exec ./target/release/audiomirror-cli daemon --signaling-port "$PORT" --peer-name "$PEER_NAME" --identity-dir "$IDENTITY_DIR"
