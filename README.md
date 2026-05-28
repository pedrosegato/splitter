# AudioMirror

Lightweight LAN audio mirror between PCs.

## Build

```
cargo build --workspace --release
```

## Try the CLI (Phase 1)

Subcommands land in Task 13 onward; the current scaffold only verifies the binary runs.

```
cargo run -p audiomirror-cli -- devices
cargo run -p audiomirror-cli -- loop --input <id> --output <id>
```
