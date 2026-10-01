use crate::{
    config::Config,
    filesystem::{atomic_write, open_regular},
    sources::{Provider, Snapshot, hash},
};
use anyhow::{Result, bail};
use pulldown_cmark::{Event, Parser, Tag};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Attachment {
    Copied {
        reference: String,
        path: String,
        sha256: String,
        bytes: u64,
    },
    Omitted {
        reference: String,
        reason: String,
    },
}

impl Attachment {
    pub fn omitted(&self) -> bool {
        matches!(self, Self::Omitted { .. })
    }
}

pub fn references(provider: Provider, record: &Value, found: &mut BTreeSet<String>) {
    match provider {
        Provider::Codex => {
            let payload = &record["payload"];
            match record["type"].as_str() {
                Some("event_msg") => match payload["type"].as_str() {
                    Some("item_completed") => {
                        let item = &payload["item"];
                        if matches!(item["type"].as_str(), Some("UserMessage" | "AgentMessage")) {
                            message_content(&item["content"], found);
                        }
                    }
                    Some("user_message" | "agent_message") => {
                        for field in ["message", "text"] {
                            if let Some(text) = payload[field].as_str() {
                                markdown(text, found);
                            }
                        }
                        for field in ["images", "local_images"] {
                            if let Some(images) = payload[field].as_array() {
                                for image in images {
                                    if let Some(reference) = image.as_str() {
                                        add(reference, found);
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                },
                Some("response_item")
                    if payload["type"] == "message"
                        && matches!(payload["role"].as_str(), Some("user" | "assistant")) =>
                {
                    message_content(&payload["content"], found)
                }
                _ => media_block(record, found),
            }
        }
        Provider::Claude => {
            if matches!(record["type"].as_str(), Some("user" | "assistant")) {
                let content = &record["message"]["content"];
                message_content(content, found);
                if let Some(blocks) = content.as_array() {
                    for block in blocks {
                        if block["type"] == "tool_result"
                            && let Some(result) = block["content"].as_array()
                        {
                            for media in result {
                                media_block(media, found);
                            }
                        }
                    }
                }
            }
        }
    }
}

fn message_content(value: &Value, found: &mut BTreeSet<String>) {
    match value {
        Value::String(text) => markdown(text, found),
        Value::Array(blocks) => {
            for block in blocks {
                if matches!(
                    block["type"].as_str(),
                    Some("text" | "Text" | "input_text" | "output_text")
                ) {
                    if let Some(text) = block["text"].as_str() {
                        markdown(text, found);
                    }
                } else {
                    media_block(block, found);
                }
            }
        }
        _ => {}
    }
}

fn media_block(value: &Value, found: &mut BTreeSet<String>) {
    if !matches!(
        value["type"].as_str(),
        Some(
            "image"
                | "Image"
                | "input_image"
                | "output_image"
                | "local_image"
                | "LocalImage"
                | "document"
                | "attachment"
                | "audio"
                | "input_audio"
                | "video"
        )
    ) {
        return;
    }
    for fields in [value, &value["source"], &value["image_url"]] {
        for key in [
            "image_path",
            "attachment_path",
            "local_image",
            "image_url",
            "path",
            "file_path",
            "local_path",
            "url",
        ] {
            if let Some(reference) = fields.get(key).and_then(Value::as_str) {
                add(reference, found);
            }
        }
    }
}

fn add(reference: &str, found: &mut BTreeSet<String>) {
    if !reference.is_empty() && !reference.starts_with("data:") {
        found.insert(reference.to_owned());
    }
}

fn markdown(text: &str, found: &mut BTreeSet<String>) {
    for event in Parser::new(text) {
        if let Event::Start(Tag::Image { dest_url, .. } | Tag::Link { dest_url, .. }) = event {
            add(&dest_url, found);
        }
    }
}

pub fn capture(
    config: &Config,
    snapshot: &Snapshot,
    directory: &Path,
    previous: &[Attachment],
) -> Result<Vec<Attachment>> {
    let mut captured = Vec::new();
    for reference in &snapshot.references {
        if let Some(prior) = carry_forward(reference, directory, previous) {
            captured.push(prior);
            continue;
        }
        let omitted = |reason: &str| Attachment::Omitted {
            reference: reference.clone(),
            reason: reason.to_owned(),
        };
        let file_uri = reference.starts_with("file://");
        let raw_path = if let Some(path) = reference.strip_prefix("file://") {
            path
        } else {
            reference
        };
        if raw_path.contains("://")
            || raw_path.starts_with("http:")
            || raw_path.starts_with("https:")
        {
            captured.push(omitted("remote_reference"));
            continue;
        }
        let path = if file_uri {
            PathBuf::from(decode_path(raw_path))
        } else {
            PathBuf::from(raw_path)
        };
        let path = if path.is_absolute() {
            path
        } else if let Some(cwd) = &snapshot.cwd {
            cwd.join(path)
        } else {
            captured.push(omitted("relative_reference_without_cwd"));
            continue;
        };
        let resolved = fs::canonicalize(&path).or_else(|error| {
            if !file_uri && raw_path.contains('%') && error.kind() == std::io::ErrorKind::NotFound {
                let decoded = PathBuf::from(decode_path(raw_path));
                let decoded = if decoded.is_absolute() {
                    decoded
                } else {
                    snapshot.cwd.as_ref().unwrap().join(decoded)
                };
                fs::canonicalize(decoded)
            } else {
                Err(error)
            }
        });
        let canonical = match resolved {
            Ok(path) => path,
            Err(error) => {
                if error.kind() == std::io::ErrorKind::NotFound {
                    captured.push(omitted("missing"));
                } else {
                    captured.push(omitted("unreadable"));
                }
                continue;
            }
        };
        if !config
            .attachment_roots
            .as_ref()
            .unwrap()
            .iter()
            .any(|root| canonical.starts_with(root))
        {
            captured.push(omitted("outside_allowed_roots"));
            continue;
        }
        let metadata = match fs::metadata(&canonical) {
            Ok(metadata) => metadata,
            Err(_) => {
                captured.push(omitted("unreadable_or_changed"));
                continue;
            }
        };
        if !metadata.is_file() {
            captured.push(omitted("not_regular_file"));
            continue;
        }
        if metadata.len() > config.max_attachment_bytes {
            captured.push(omitted("too_large"));
            continue;
        }
        let bytes = match read_limited(&canonical, config.max_attachment_bytes) {
            Ok(bytes) => bytes,
            Err(_) => {
                captured.push(omitted("unreadable_or_changed"));
                continue;
            }
        };
        let unchanged_time = fs::metadata(&canonical)
            .and_then(|m| m.modified())
            .and_then(|after| metadata.modified().map(|before| before == after));
        if bytes.len() as u64 != metadata.len() || !matches!(unchanged_time, Ok(true)) {
            captured.push(omitted("changed_during_capture"));
            continue;
        }
        let digest = hash(&bytes);
        let extension = canonical
            .extension()
            .and_then(|s| s.to_str())
            .filter(|s| s.len() <= 12 && s.bytes().all(|b| b.is_ascii_alphanumeric()))
            .unwrap_or("bin")
            .to_ascii_lowercase();
        let relative = format!("attachments/{digest}.{extension}");
        atomic_write(&directory.join(&relative), &bytes)?;
        captured.push(Attachment::Copied {
            reference: reference.clone(),
            path: relative,
            sha256: digest,
            bytes: bytes.len() as u64,
        });
    }
    Ok(captured)
}

fn carry_forward(reference: &str, directory: &Path, previous: &[Attachment]) -> Option<Attachment> {
    for attachment in previous {
        if let Attachment::Copied {
            reference: old,
            path,
            sha256,
            bytes,
        } = attachment
        {
            if old != reference {
                continue;
            }
            let path = Path::new(path);
            if path.is_absolute()
                || path
                    .components()
                    .any(|c| !matches!(c, Component::Normal(_)))
                || !path.starts_with("attachments")
            {
                continue;
            }
            let path = directory.join(path);
            if let Ok(file) = open_regular(&path)
                && file.metadata().is_ok_and(|m| m.len() == *bytes)
            {
                let mut reader = file.take(bytes.saturating_add(1));
                let mut digest = Sha256::new();
                let mut length = 0;
                let mut buffer = [0; 65536];
                loop {
                    match reader.read(&mut buffer) {
                        Ok(0) => {
                            if length == *bytes && format!("{:x}", digest.finalize()) == *sha256 {
                                return Some(attachment.clone());
                            }
                            break;
                        }
                        Err(_) => break,
                        Ok(count) => {
                            length += count as u64;
                            digest.update(&buffer[..count]);
                        }
                    }
                }
            }
        }
    }
    None
}

fn read_limited(path: &Path, cap: u64) -> Result<Vec<u8>> {
    let file = open_regular(path)?;
    if !file.metadata()?.is_file() {
        bail!("attachment is not a regular file");
    }
    let mut content = Vec::new();
    file.take(cap.saturating_add(1)).read_to_end(&mut content)?;
    if content.len() as u64 > cap {
        bail!("attachment exceeds cap");
    }
    Ok(content)
}

fn decode_path(value: &str) -> String {
    let mut bytes = Vec::new();
    let mut remaining = value.as_bytes();
    while !remaining.is_empty() {
        if remaining.len() >= 3
            && remaining[0] == b'%'
            && let (Some(a), Some(b)) = (
                (remaining[1] as char).to_digit(16),
                (remaining[2] as char).to_digit(16),
            )
        {
            bytes.push((a * 16 + b) as u8);
            remaining = &remaining[3..];
            continue;
        }
        bytes.push(remaining[0]);
        remaining = &remaining[1..];
    }
    String::from_utf8_lossy(&bytes).into_owned()
}
