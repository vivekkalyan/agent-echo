# Agent conversation archives

These terms describe the history preserved by agent-echo and read by obsidian-conversation-viewer.

## Language

**Conversation**:
A recorded exchange between a user and a coding agent, including associated tool activity. This is the unit a person browses in the viewer.

**Native session**:
A session identified and stored by Codex or Claude Code. Its provider-issued identity connects source records to a conversation.

**Source transcript**:
An original JSONL file written by the coding agent. It is the input to collection and may include events beyond the visible conversation.

**Archived transcript**:
A captured copy of a source transcript stored as gzip-compressed JSONL. It preserves original records independently of the readable interpretation.

**Collector**:
The agent-echo program that reads native session storage and preserves transcripts and eligible attachments. It does not resume conversations or write back to the coding agent.

**Viewer**:
The obsidian-conversation-viewer plugin that interprets conversation records for browsing and inspection. Using agent-echo is not a prerequisite for using the viewer.

**Producer**:
The machine whose native session storage a collector reads. A laptop controlling a session on the devbox does not become that session's producer.

**Attachment**:
A file associated with a conversation, such as an image, report, or recording. An attachment can be archived, deliberately excluded, or missing from the source machine.

**Collection**:
The act of capturing native session data into a local archive. Successful collection does not by itself mean the archive has reached another machine.

**Sync**:
The propagation of vault files through Obsidian Sync. It is distinct from collection and from an independent versioned backup.
