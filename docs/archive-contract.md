# Agent Echo archive contract v1

Each source transcript has a fixed `.jsonl.gz` file and a sibling `.meta.json` file. The sidecar basename equals the gzip basename without `.jsonl.gz`. Gzip decompression returns original source bytes. The sidecar is optional metadata, never a prerequisite for opening the transcript.

## Layout

`<archive-root>/<provider>/<safe-conversation-id>/<source-id>.jsonl.gz`
`<archive-root>/<provider>/<safe-conversation-id>/<source-id>.meta.json`
`<archive-root>/<provider>/<safe-conversation-id>/attachments/<sha256>.<extension>`

Provider is `codex` or `claude`. Source ID is lowercase hexadecimal SHA-256 of four UTF-8 strings in order: `agent-echo-source-v1`, producer identity, provider, and logical native file identity. Prefix each string with its byte length encoded as an unsigned 64-bit big-endian integer. Codex logical file identity uses the rollout basename, independent of sessions versus archived_sessions. Claude uses the projects-relative path, including project and subagent directories and superseded suffixes. Preserve every distinct native source file. Report same-key differing live files as conflicts rather than selecting a winner. Safe conversation IDs use ASCII alphanumeric, hyphen and underscore unchanged when nonempty and at most 120 bytes; other IDs use a SHA-256 path component. Metadata contains the original conversation ID. The directory chosen at first capture stays fixed if later native metadata changes that ID, preventing duplicate snapshots. Readers use verified metadata for relationships rather than assuming directory names are current identities.

Producer is a stable user-configured identity unique per machine. It is metadata and part of the opaque source ID, not a top-level directory. A configuration reused on a different machine must get a different producer value. Config and local state are outside the vault. No shared mutable vault index exists.

## Metadata

All fields below are required, with null for absent parent. `captured_at` is Unix seconds. Attachment paths are relative to this sidecar directory, never absolute or containing parent traversal.

```json
{
  "schema_version": 1,
  "provider": "codex",
  "producer": "example-machine",
  "source_id": "64 lowercase hex characters",
  "conversation_id": "example-conversation",
  "parent_conversation_id": null,
  "source_path": "/example/native/rollout.jsonl",
  "source_name": "rollout.jsonl",
  "transcript_file": "64 lowercase hex characters.jsonl.gz",
  "raw_sha256": "64 lowercase hex characters",
  "raw_bytes": 123,
  "captured_at": 0,
  "attachments": [
    {
      "reference": "/example/image.png",
      "status": "copied",
      "path": "attachments/64-lowercase-hex.png",
      "sha256": "64 lowercase hex characters",
      "bytes": 12
    },
    { "reference": "/example/video.mp4", "status": "omitted", "reason": "too_large" }
  ]
}
```

The example shows field shape, not a valid hash fixture. A viewer validates the entire sidecar at its boundary, verifies `raw_bytes` and `raw_sha256` against decompressed bytes, and ignores stale or malformed metadata with a visible diagnostic. It never follows unverified attachment mappings. Native transcript content remains readable during sidecar delivery or repair. Raw `.jsonl` files and arbitrary `.jsonl.gz` files need no sidecars. Sibling valid metadata with the same provider, producer and conversation ID identifies related source pages. Claude subagent conversation IDs include their agent identity; their parent is the native session ID.

## Publication

Capture a stable full source snapshot read-only. A parseable final record without a newline is complete. A malformed partial tail postpones capture and retains the previous archive. Unknown and malformed interior newline-terminated records remain original bytes. Deterministic gzip uses mtime zero. Compare content hashes, not only stat fingerprints, before deciding that content is unchanged.

Copy authorized attachments first, publish gzip by a same-directory atomic replacement, then atomically replace metadata. If a crash leaves stale metadata, the gzip remains independently readable and the next capture repairs the sidecar. Previously copied attachments remain available when their original files disappear. Source deletion and storage pressure never prune archived files. The local status report distinguishes capture failures from omissions and never claims remote Sync success.

Sidecars are limited to 2,000,000 bytes. If attachment mappings exceed that limit, the collector publishes metadata without mappings and reports the omission. If even base metadata exceeds the limit, it skips the sidecar update and reports a diagnostic. Optional metadata never blocks preservation of a valid transcript.

The default separate attachment cap is 20,000,000 bytes. The default compressed transcript cap is 200,000,000 bytes. Oversized transcripts produce a capture failure while retaining any previous snapshot; no implicit splitting or truncation occurs. Inline media remains part of original source bytes. Attachment discovery never fetches remote URLs or follows arbitrary tool text into the filesystem. Explicit local media/reference fields and known markdown attachment links are candidates, subject to configured allowed roots and symlink containment.
