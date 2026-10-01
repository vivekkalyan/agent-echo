# Agent Echo

Agent Echo will preserve complete Codex and Claude Code conversations as compressed native transcripts. This repository currently contains the Rust command scaffold and development checks. Transcript collection and attachment support will follow in separate changes.

## Develop

Use Rust 1.91 or newer. CI uses Rust 1.91.1 on Linux and macOS.

```sh
cargo run --locked -- --help
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo build --locked --release
```

The scaffold exposes help and version information. The test command is wired into CI; there are no behavior tests yet. Add tests alongside the features they exercise.

Build output and local scratch files are ignored. Keep real conversations and machine configuration out of this source repository.

MIT licensed.
