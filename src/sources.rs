use crate::{
    attachments,
    config::{Config, checked_path},
    filesystem::open_regular,
    metadata::NativeSummary,
};
use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};
use walkdir::WalkDir;

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Codex,
    Claude,
}

impl Provider {
    pub fn name(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
        }
    }
}

pub struct Source {
    pub provider: Provider,
    pub logical_path: String,
    pub paths: Vec<PathBuf>,
}

pub struct Snapshot {
    pub bytes: Vec<u8>,
    pub digest: String,
    pub conversation: String,
    pub parent: Option<String>,
    pub references: BTreeSet<String>,
    pub cwd: Option<PathBuf>,
    pub diagnostics: Vec<String>,
    pub summary: NativeSummary,
}

pub fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn source_id(producer: &str, source: &Source) -> String {
    let mut digest = Sha256::new();
    for part in [
        "agent-echo-source-v1",
        producer,
        source.provider.name(),
        &source.logical_path,
    ] {
        digest.update((part.len() as u64).to_be_bytes());
        digest.update(part.as_bytes());
    }
    format!("{digest:x}", digest = digest.finalize())
}

pub fn safe_component(value: &str) -> String {
    if !value.is_empty()
        && value.len() <= 120
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    {
        value.to_owned()
    } else {
        format!("sha256-{}", hash(value.as_bytes()))
    }
}

pub fn discover(config: &Config) -> (Vec<Source>, Vec<String>) {
    let mut found: BTreeMap<(Provider, String), Vec<PathBuf>> = BTreeMap::new();
    let mut errors = Vec::new();
    for (provider, root) in [
        (Provider::Codex, config.codex_home.join("sessions")),
        (Provider::Codex, config.codex_home.join("archived_sessions")),
        (Provider::Claude, config.claude_home.join("projects")),
    ] {
        if let Err(error) = checked_path(&root) {
            errors.push(format!("{error:#}"));
            continue;
        }
        if !root.exists() {
            continue;
        }
        for entry in WalkDir::new(&root).follow_links(false).sort_by_file_name() {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    errors.push(error.to_string());
                    continue;
                }
            };
            let name = entry.file_name().to_string_lossy();
            if entry.file_type().is_symlink() {
                errors.push(format!(
                    "source symlink is not permitted: {}",
                    entry.path().display()
                ));
                continue;
            }
            let eligible = match provider {
                Provider::Codex => name.ends_with(".jsonl"),
                Provider::Claude => claude_transcript_name(&name),
            };
            if !eligible {
                continue;
            }
            if !entry.file_type().is_file() {
                errors.push(format!(
                    "source is not a regular file: {}",
                    entry.path().display()
                ));
                continue;
            }
            let logical = match provider {
                Provider::Codex => name.into_owned(),
                Provider::Claude => entry
                    .path()
                    .strip_prefix(&root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/"),
            };
            found
                .entry((provider, logical))
                .or_default()
                .push(entry.path().to_owned());
        }
    }
    (
        found
            .into_iter()
            .map(|((provider, logical_path), paths)| Source {
                provider,
                logical_path,
                paths,
            })
            .collect(),
        errors,
    )
}

fn claude_transcript_name(name: &str) -> bool {
    let identifier = |value: &str| {
        !value.is_empty()
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    };
    let timestamp = |value: &str| !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit());
    if let Some(stem) = name.strip_suffix(".jsonl") {
        if let Some((session, suffix)) = stem.split_once(".orphaned-") {
            return identifier(session)
                && suffix
                    .split_once('-')
                    .is_some_and(|(time, suffix)| timestamp(time) && identifier(suffix));
        }
        return identifier(stem);
    }
    name.split_once(".jsonl.superseded-")
        .is_some_and(|(session, time)| identifier(session) && timestamp(time))
}

fn stable_read(path: &Path) -> Result<Vec<u8>> {
    checked_path(path)?;
    let mut file = open_regular(path).with_context(|| format!("open {}", path.display()))?;
    let before = file.metadata()?;
    if !before.is_file() {
        bail!("source is not a regular file");
    }
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(
        &mut std::io::Read::take(&mut file, before.len().saturating_add(1)),
        &mut bytes,
    )?;
    let after = file.metadata()?;
    let current = fs::metadata(path)?;
    if before.len() != bytes.len() as u64
        || before.len() != after.len()
        || before.modified()? != after.modified()?
        || before.len() != current.len()
        || before.modified()? != current.modified()?
    {
        bail!("source changed during capture; retry next collection");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if before.dev() != current.dev() || before.ino() != current.ino() {
            bail!("source replaced during capture; retry next collection");
        }
    }
    Ok(bytes)
}

pub fn snapshot(source: &Source) -> Result<Snapshot> {
    let bytes = stable_read(&source.paths[0])?;
    let digest = hash(&bytes);
    for path in &source.paths[1..] {
        if hash(&stable_read(path)?) != digest {
            bail!(
                "conflicting live files claim the same source identity: {}",
                source.logical_path
            );
        }
    }
    let mut conversation = None;
    let mut parent = None;
    let mut agent = None;
    let mut cwd = None;
    let mut references = BTreeSet::new();
    let mut diagnostics = Vec::new();
    let mut unknown_types = BTreeSet::new();
    let mut summary = NativeSummary::new(source.provider, &source.logical_path);
    let lines: Vec<_> = bytes.split(|b| *b == b'\n').collect();
    let final_record = lines
        .iter()
        .rposition(|line| !line.iter().all(u8::is_ascii_whitespace));
    for (index, line) in lines.iter().enumerate() {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let record: Value = match serde_json::from_slice(line) {
            Ok(record) => record,
            Err(error) => {
                if Some(index) == final_record {
                    bail!(
                        "incomplete or malformed final JSON record at line {}: {error}",
                        index + 1
                    );
                }
                diagnostics.push(format!(
                    "malformed interior JSON preserved at line {}",
                    index + 1
                ));
                continue;
            }
        };
        let payload = record.get("payload").unwrap_or(&record);
        summary.observe(&record);
        match source.provider {
            Provider::Codex => {
                if record.get("type").and_then(Value::as_str) == Some("session_meta") {
                    conversation = conversation.or_else(|| string(payload, "id"));
                    parent = parent
                        .or_else(|| string(payload, "forked_from_id"))
                        .or_else(|| string(payload, "parent_thread_id"));
                    cwd = cwd.or_else(|| string(payload, "cwd").map(PathBuf::from));
                }
            }
            Provider::Claude => {
                conversation = conversation.or_else(|| string(&record, "sessionId"));
                agent = agent.or_else(|| string(&record, "agentId"));
                cwd = cwd.or_else(|| string(&record, "cwd").map(PathBuf::from));
            }
        }
        if !record.is_object() || record.get("type").and_then(Value::as_str).is_none() {
            diagnostics.push(format!(
                "unknown record shape preserved at line {}",
                index + 1
            ));
        } else if let Some(kind) = record.get("type").and_then(Value::as_str) {
            let known = match source.provider {
                Provider::Codex => matches!(
                    kind,
                    "session_meta" | "response_item" | "event_msg" | "turn_context" | "compacted"
                ),
                Provider::Claude => matches!(
                    kind,
                    "user"
                        | "assistant"
                        | "system"
                        | "progress"
                        | "summary"
                        | "file-history-snapshot"
                        | "queue-operation"
                        | "last-prompt"
                        | "custom-title"
                        | "ai-title"
                        | "agent-name"
                        | "tag"
                ),
            };
            if !known && unknown_types.insert(kind.to_owned()) {
                diagnostics.push(format!(
                    "unknown record type {kind:?} preserved at line {}",
                    index + 1
                ));
            }
        }
        attachments::references(source.provider, &record, &mut references);
    }
    let fallback = source.paths[0]
        .file_name()
        .unwrap()
        .to_string_lossy()
        .trim_end_matches(".jsonl")
        .to_owned();
    let mut conversation = conversation.unwrap_or(fallback);
    if source.provider == Provider::Claude {
        let components: Vec<_> = source.logical_path.split('/').collect();
        if let Some(index) = components.iter().position(|part| *part == "subagents") {
            let session = if index > 0 {
                components[index - 1].to_owned()
            } else {
                conversation.clone()
            };
            let identity = agent.unwrap_or_else(|| {
                source.paths[0]
                    .file_stem()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            });
            parent = Some(session.clone());
            conversation = format!("{session}:{identity}");
        }
    }
    Ok(Snapshot {
        bytes,
        digest,
        conversation,
        parent,
        references,
        cwd,
        diagnostics,
        summary,
    })
}

fn string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}
