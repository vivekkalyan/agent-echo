use crate::{filesystem::open_regular, sources::Provider};
use serde::Serialize;
use serde_json::Value;
use std::{collections::BTreeMap, io::Read, path::Path};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

const MAX_INDEX_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    Conversation,
    Subagent,
    Unknown,
}

#[derive(Serialize)]
pub struct DisplayMetadata {
    version: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    first_prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    first_message_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_message_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    version_created_at: Option<u64>,
    source_kind: SourceKind,
}

pub struct NativeSummary {
    provider: Provider,
    display: DisplayMetadata,
    custom_title: Option<String>,
    ai_title: Option<String>,
}

pub fn codex_titles(home: &Path) -> BTreeMap<String, String> {
    let read = || -> Option<BTreeMap<String, String>> {
        let file = open_regular(&home.join("session_index.jsonl")).ok()?;
        if file.metadata().ok()?.len() > MAX_INDEX_BYTES {
            return None;
        }
        let mut bytes = Vec::new();
        file.take(MAX_INDEX_BYTES + 1)
            .read_to_end(&mut bytes)
            .ok()?;
        if bytes.len() as u64 > MAX_INDEX_BYTES {
            return None;
        }
        let mut titles = BTreeMap::<String, (u64, String)>::new();
        for line in bytes.split(|b| *b == b'\n') {
            let Ok(record) = serde_json::from_slice::<Value>(line) else {
                continue;
            };
            let Some(id) = record["id"].as_str().filter(|s| !s.is_empty()) else {
                continue;
            };
            let (Some(title), Some(updated)) = (
                text(&record["thread_name"], 200),
                timestamp(&record["updated_at"]),
            ) else {
                continue;
            };
            if titles.get(id).is_none_or(|(time, _)| updated >= *time) {
                titles.insert(id.to_owned(), (updated, title));
            }
        }
        Some(
            titles
                .into_iter()
                .map(|(id, (_, title))| (id, title))
                .collect(),
        )
    };
    read().unwrap_or_default()
}

impl NativeSummary {
    pub fn new(provider: Provider, logical_path: &str) -> Self {
        Self {
            provider,
            display: DisplayMetadata {
                version: 1,
                title: None,
                first_prompt: None,
                cwd: None,
                first_message_at: None,
                last_message_at: None,
                version_created_at: None,
                source_kind: if provider == Provider::Claude
                    && logical_path.split('/').any(|part| part == "subagents")
                {
                    SourceKind::Subagent
                } else {
                    SourceKind::Unknown
                },
            },
            custom_title: None,
            ai_title: None,
        }
    }

    pub fn observe(&mut self, record: &Value) {
        let kind = record["type"].as_str().unwrap_or_default();
        let payload = record.get("payload").unwrap_or(record);
        let mut user_text = None;
        let mut assistant = false;
        match self.provider {
            Provider::Codex => {
                if kind == "session_meta" {
                    self.display.cwd = self
                        .display
                        .cwd
                        .take()
                        .or_else(|| text(&payload["cwd"], 4096));
                    self.display.version_created_at = self
                        .display
                        .version_created_at
                        .or_else(|| timestamp(&payload["timestamp"]));
                    if payload["thread_source"] == "subagent"
                        || payload["source"].get("subagent").is_some()
                    {
                        self.display.source_kind = SourceKind::Subagent;
                    } else if matches!(self.display.source_kind, SourceKind::Unknown)
                        && payload["source"].is_string()
                    {
                        self.display.source_kind = SourceKind::Conversation;
                    }
                }
                if kind == "event_msg" {
                    let prose = payload.get("message").unwrap_or(&payload["text"]);
                    match payload["type"].as_str() {
                        Some("user_message") => user_text = prompt(prose),
                        Some("agent_message") => assistant = !content_text(prose).trim().is_empty(),
                        Some("item_completed") => {
                            let item = &payload["item"];
                            match item["type"].as_str() {
                                Some("UserMessage") => user_text = prompt(&item["content"]),
                                Some("AgentMessage") => {
                                    assistant = !content_text(&item["content"]).trim().is_empty()
                                }
                                _ => {}
                            }
                        }
                        _ => {}
                    }
                }
                if kind == "response_item" && payload["type"] == "message" {
                    match payload["role"].as_str() {
                        Some("user") => user_text = prompt(&payload["content"]),
                        Some("assistant") => {
                            assistant = !content_text(&payload["content"]).trim().is_empty()
                        }
                        _ => {}
                    }
                }
            }
            Provider::Claude => {
                self.display.cwd = self
                    .display
                    .cwd
                    .take()
                    .or_else(|| text(&record["cwd"], 4096));
                if record["isSidechain"] == true || record["agentId"].is_string() {
                    self.display.source_kind = SourceKind::Subagent;
                } else if matches!(self.display.source_kind, SourceKind::Unknown)
                    && record["sessionId"].is_string()
                {
                    self.display.source_kind = SourceKind::Conversation;
                }
                match kind {
                    "custom-title" => {
                        if let Some(title) = text(&record["customTitle"], 200) {
                            self.custom_title = Some(title);
                        }
                    }
                    "ai-title" => {
                        if let Some(title) = text(&record["aiTitle"], 200) {
                            self.ai_title = Some(title);
                        }
                    }
                    "user" if record["isMeta"] != true && record["isCompactSummary"] != true => {
                        user_text = prompt(&record["message"]["content"]);
                    }
                    "assistant"
                        if record["isMeta"] != true && record["isCompactSummary"] != true =>
                    {
                        assistant = !content_text(&record["message"]["content"])
                            .trim()
                            .is_empty()
                    }
                    _ => {}
                }
            }
        }
        if user_text.is_some() || assistant {
            if self.display.first_prompt.is_none() {
                self.display.first_prompt = user_text;
            }
            if let Some(time) = timestamp(&record["timestamp"]) {
                self.display.first_message_at =
                    Some(self.display.first_message_at.map_or(time, |v| v.min(time)));
                self.display.last_message_at =
                    Some(self.display.last_message_at.map_or(time, |v| v.max(time)));
            }
        }
    }

    pub fn finish(mut self, named_title: Option<&String>) -> DisplayMetadata {
        self.display.title = named_title.cloned().or(self.custom_title).or(self.ai_title);
        if self.provider == Provider::Claude {
            self.display.version_created_at = self.display.first_message_at;
        }
        self.display
    }
}

fn timestamp(value: &Value) -> Option<u64> {
    let parsed = OffsetDateTime::parse(value.as_str()?, &Rfc3339).ok()?;
    let nanos = u128::try_from(parsed.unix_timestamp_nanos()).ok()?;
    let millis = u64::try_from(nanos / 1_000_000).ok()?;
    (millis <= 8_640_000_000_000_000).then_some(millis)
}

fn text(value: &Value, limit: usize) -> Option<String> {
    normalize(value.as_str()?, limit)
}

fn normalize(value: &str, limit: usize) -> Option<String> {
    let normalized: String = value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(limit)
        .collect();
    (!normalized.is_empty()).then_some(normalized)
}

fn content_text(value: &Value) -> String {
    if let Some(text) = value.as_str() {
        text.to_owned()
    } else {
        value
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter(|part| {
                matches!(
                    part["type"].as_str(),
                    Some("text" | "input_text" | "output_text" | "Text")
                )
            })
            .filter_map(|part| part["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn prompt(value: &Value) -> Option<String> {
    let mut text = content_text(value);
    for tag in [
        "system-reminder",
        "environment_context",
        "instructions",
        "system",
        "developer",
        "context",
        "app-context",
        "skills_instructions",
        "permissions instructions",
        "collaboration_mode",
    ] {
        let opening = format!("<{tag}");
        let closing = format!("</{tag}>");
        let mut offset = 0;
        while let Some(relative) = text[offset..].to_ascii_lowercase().find(&opening) {
            let start = offset + relative;
            let boundary = text.as_bytes().get(start + opening.len()).copied();
            if boundary != Some(b'>') && !boundary.is_some_and(|b| b.is_ascii_whitespace()) {
                offset = start + opening.len();
                continue;
            }
            let end = text[start + opening.len()..]
                .to_ascii_lowercase()
                .find(&closing)
                .map_or(text.len(), |end| {
                    start + opening.len() + end + closing.len()
                });
            text.replace_range(start..end, " ");
        }
    }
    let lower = text.trim_start().to_ascii_lowercase();
    if [
        "# agents.md instructions",
        "<skill>",
        "<turn_aborted>",
        "[request interrupted",
        "<local-command-caveat>",
        "<command-name>",
    ]
    .iter()
    .any(|prefix| lower.starts_with(prefix))
    {
        return None;
    }
    normalize(&text, 280)
}
