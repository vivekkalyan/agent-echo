# Agent Echo

Back up complete Codex and Claude Code conversations as native `.jsonl.gz` files. Agent Echo reads source files without changing them. Use ordinary gzip tools or Obsidian Conversation Viewer to read the archive.

## Collect conversations

Build and install the command with Rust 1.91 or newer.

```sh
cargo install --path . --locked
```

Copy `config.example.toml` to your configuration directory. Set a unique producer identity for each machine, the archive destination inside your vault, and a state directory outside the vault. Configure the native application homes you want to collect.

```sh
agent-echo --config ~/.config/agent-echo/config.toml check-config
agent-echo --config ~/.config/agent-echo/config.toml collect
agent-echo --config ~/.config/agent-echo/config.toml status
```

Collection writes the latest complete snapshot of each native source file. A changing file or malformed final JSON record postpones that file until a later run, retaining any previous snapshot. Malformed interior records remain preserved. An unchanged transcript keeps the same archive bytes and modification time. Deleting a native source does not delete its archive. Agent Echo never prunes old conversations to make room.

The command reports capture failures. A successful capture means that the local archive was written. It does not prove that Obsidian Sync uploaded it.

## Schedule and synchronize

Use the [scheduling examples](docs/scheduling.md) to collect at startup and hourly. Installation and scheduling belong in your machine configuration or dotfiles.

Enable syncing of other file types in Obsidian Sync so gzip transcripts and JSON metadata can propagate. Keep one Sync engine per device. Desktop Sync uploads only while the app runs; Headless Sync can run independently. Agent Echo does not install or configure Sync.

## What gets preserved

Full transcript bytes include tool activity, internal context, and inline media. Separate referenced files are not copied yet; attachment preservation will follow in a separate change.

Each source file has one current gzip archive and an optional hash-bound metadata sidecar. A native conversation may contain multiple source pages. Every page is preserved. Source rewrites replace the current snapshot, so messages genuinely removed by the source can disappear from that snapshot. There is no separate revision-history system.

The default compressed-file cap is 200 MB. Oversized captures fail without replacing an existing archive. Obsidian Sync may retain its own older versions, which consume additional quota.

See the [archive contract](docs/archive-contract.md) for identity, metadata, and publication rules. Archives contain private conversation data. Keep your archive destination separate from this public source repository.

## Develop

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo build --locked --release
```

The collector targets Linux and macOS. It holds one source transcript in memory during capture.

Tests use synthetic native files in temporary directories. They do not need real Codex or Claude sessions. CI checks Linux and macOS. Release builds produce binaries for Linux x86-64 and macOS arm64 and x86-64.

MIT licensed.
