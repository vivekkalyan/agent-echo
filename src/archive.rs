use crate::{
    attachments::{self, Attachment},
    config::{Config, checked_path},
    filesystem::{atomic_write, open_lock, open_regular},
    sources::{self, Source},
};
use anyhow::{Context, Result, bail};
use flate2::{Compression, GzBuilder, read::MultiGzDecoder};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use walkdir::WalkDir;

const MAX_METADATA_BYTES: u64 = 2_000_000;

#[derive(Deserialize, Serialize)]
struct Metadata {
    schema_version: u32,
    provider: String,
    producer: String,
    source_id: String,
    conversation_id: String,
    parent_conversation_id: Option<String>,
    source_path: PathBuf,
    source_name: String,
    transcript_file: String,
    raw_sha256: String,
    raw_bytes: u64,
    captured_at: u64,
    attachments: Vec<Attachment>,
}

#[derive(Default, Deserialize, Serialize)]
pub struct Report {
    pub producer: String,
    pub archive_root: PathBuf,
    pub state_dir: PathBuf,
    pub completed_at: Option<u64>,
    pub discovered: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub errors: usize,
    #[serde(default)]
    pub omissions: usize,
    pub sources: Vec<SourceReport>,
    pub diagnostics: Vec<String>,
    #[serde(default)]
    pub fatal: Option<String>,
}

#[derive(Deserialize, Serialize)]
pub struct SourceReport {
    source: PathBuf,
    source_id: String,
    outcome: String,
    archive: Option<PathBuf>,
    #[serde(default)]
    omissions: usize,
    diagnostics: Vec<String>,
    error: Option<String>,
}

fn report_path(config: &Config) -> PathBuf {
    config.state_dir.join(format!(
        "{}.report.json",
        sources::hash(config.producer.as_bytes())
    ))
}

pub fn status(config: &Config) -> Result<Report> {
    let path = report_path(config);
    checked_path(&path)?;
    if path.exists() {
        let report: Report = serde_json::from_reader(open_regular(&path)?)?;
        if report.producer != config.producer || report.archive_root != config.archive_root {
            bail!("last report belongs to a different configuration; run collect");
        }
        Ok(report)
    } else {
        Ok(Report {
            producer: config.producer.clone(),
            archive_root: config.archive_root.clone(),
            state_dir: config.state_dir.clone(),
            diagnostics: vec!["No local collection has completed for this producer.".into()],
            ..Report::default()
        })
    }
}

pub fn collect(config: &Config) -> Result<Report> {
    checked_path(&config.state_dir)?;
    let lock_path = config.state_dir.join(format!(
        "{}.lock",
        sources::hash(config.producer.as_bytes())
    ));
    checked_path(&lock_path)?;
    let lock = open_lock(&lock_path)?;
    lock.try_lock()
        .context("another collection for this producer is running")?;
    let (sources, discovery_errors) = sources::discover(config);
    let mut report = Report {
        producer: config.producer.clone(),
        archive_root: config.archive_root.clone(),
        state_dir: config.state_dir.clone(),
        discovered: sources.len(),
        errors: discovery_errors.len(),
        diagnostics: discovery_errors,
        ..Report::default()
    };
    let published = match published_paths(config) {
        Ok(published) => published,
        Err(error) => {
            report.errors += 1;
            report.fatal = Some(format!("archive discovery failed: {error:#}"));
            report.completed_at = Some(now());
            atomic_write(&report_path(config), &serde_json::to_vec_pretty(&report)?)?;
            return Ok(report);
        }
    };
    for source in sources {
        let id = sources::source_id(&config.producer, &source);
        let mut entry = SourceReport {
            source: source.paths[0].clone(),
            source_id: id.clone(),
            outcome: "error".into(),
            archive: None,
            omissions: 0,
            diagnostics: Vec::new(),
            error: None,
        };
        match capture(config, &source, &id, published.get(&id), &mut entry) {
            Ok(changed) => {
                entry.outcome = if changed {
                    report.updated += 1;
                    "updated"
                } else {
                    report.unchanged += 1;
                    "unchanged"
                }
                .into();
                report.omissions += entry.omissions;
            }
            Err(error) => {
                report.errors += 1;
                entry.error = Some(format!("{error:#}"));
            }
        }
        report.sources.push(entry);
    }
    report.completed_at = Some(now());
    atomic_write(&report_path(config), &serde_json::to_vec_pretty(&report)?)?;
    Ok(report)
}

fn published_paths(config: &Config) -> Result<BTreeMap<String, Vec<PathBuf>>> {
    let mut paths: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    checked_path(&config.archive_root)?;
    if !config.archive_root.exists() {
        return Ok(paths);
    }
    for entry in WalkDir::new(&config.archive_root).follow_links(false) {
        let entry = entry?;
        if let Some(id) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.strip_suffix(".jsonl.gz"))
            && id.len() == 64
            && id.bytes().all(|b| b.is_ascii_hexdigit())
        {
            paths
                .entry(id.to_owned())
                .or_default()
                .push(entry.path().to_owned());
        }
    }
    Ok(paths)
}

fn capture(
    config: &Config,
    source: &Source,
    id: &str,
    published: Option<&Vec<PathBuf>>,
    entry: &mut SourceReport,
) -> Result<bool> {
    let snapshot = sources::snapshot(source)?;
    entry.diagnostics = snapshot.diagnostics.clone();
    let archive = match published.map(Vec::as_slice) {
        Some([path]) => path.clone(),
        Some(_) => {
            bail!("multiple existing archives claim source ID {id}; resolve the duplicate paths")
        }
        None => config
            .archive_root
            .join(source.provider.name())
            .join(sources::safe_component(&snapshot.conversation))
            .join(format!("{id}.jsonl.gz")),
    };
    entry.archive = Some(archive.clone());
    checked_path(&archive)?;
    let directory = archive.parent().unwrap();
    let sidecar = directory.join(format!("{id}.meta.json"));
    checked_path(&sidecar)?;
    let previous = read_metadata(&sidecar, config, source, id);
    let current = previous
        .as_ref()
        .filter(|metadata| archive_matches(&archive, metadata));
    let compressed = if current.is_some_and(|m| m.raw_sha256 == snapshot.digest) {
        None
    } else {
        let mut encoder = GzBuilder::new()
            .mtime(0)
            .write(Vec::new(), Compression::default());
        encoder.write_all(&snapshot.bytes)?;
        Some(encoder.finish()?)
    };
    let compressed_bytes = match &compressed {
        Some(bytes) => bytes.len() as u64,
        None => open_regular(&archive)?.metadata()?.len(),
    };
    if compressed_bytes > config.max_compressed_transcript_bytes {
        bail!(
            "compressed transcript is {} bytes, exceeding max_compressed_transcript_bytes={}; previous archive retained",
            compressed_bytes,
            config.max_compressed_transcript_bytes
        );
    }
    let attachments = attachments::capture(
        config,
        &snapshot,
        directory,
        previous
            .as_ref()
            .map(|m| m.attachments.as_slice())
            .unwrap_or(&[]),
    )?;
    entry.omissions = attachments.iter().filter(|a| a.omitted()).count();
    let captured_at = current
        .filter(|m| m.raw_sha256 == snapshot.digest)
        .map(|m| m.captured_at)
        .unwrap_or_else(now);
    let mut metadata = Metadata {
        schema_version: 1,
        provider: source.provider.name().into(),
        producer: config.producer.clone(),
        source_id: id.into(),
        conversation_id: snapshot.conversation,
        parent_conversation_id: snapshot.parent,
        source_path: source.paths[0].clone(),
        source_name: source.paths[0]
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned(),
        transcript_file: format!("{id}.jsonl.gz"),
        raw_sha256: snapshot.digest,
        raw_bytes: snapshot.bytes.len() as u64,
        captured_at,
        attachments,
    };
    let mut metadata_bytes = serde_json::to_vec_pretty(&metadata)?;
    if metadata_bytes.len() as u64 > MAX_METADATA_BYTES {
        entry.omissions = metadata.attachments.len();
        entry.diagnostics.push(format!("metadata exceeded {MAX_METADATA_BYTES} bytes; {} attachment mappings omitted from sidecar", metadata.attachments.len()));
        metadata.attachments.clear();
        metadata_bytes = serde_json::to_vec_pretty(&metadata)?;
    }
    let changed_archive = match compressed {
        Some(bytes) => atomic_write(&archive, &bytes)?,
        None => false,
    };
    let changed_metadata = if metadata_bytes.len() as u64 <= MAX_METADATA_BYTES {
        atomic_write(&sidecar, &metadata_bytes)?
    } else {
        entry.diagnostics.push(format!("base metadata exceeded {MAX_METADATA_BYTES} bytes; sidecar publication skipped and any previous sidecar retained"));
        false
    };
    Ok(changed_archive || changed_metadata)
}

fn read_metadata(sidecar: &Path, config: &Config, source: &Source, id: &str) -> Option<Metadata> {
    let file = open_regular(sidecar).ok()?;
    if file.metadata().ok()?.len() > MAX_METADATA_BYTES {
        return None;
    }
    let metadata: Metadata = serde_json::from_reader(file.take(MAX_METADATA_BYTES + 1)).ok()?;
    if metadata.schema_version != 1
        || metadata.provider != source.provider.name()
        || metadata.producer != config.producer
        || metadata.source_id != id
        || metadata.transcript_file != format!("{id}.jsonl.gz")
        || metadata.conversation_id.is_empty()
        || metadata.raw_sha256.len() != 64
        || !metadata
            .raw_sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return None;
    }
    Some(metadata)
}

fn archive_matches(archive: &Path, metadata: &Metadata) -> bool {
    let Ok(file) = open_regular(archive) else {
        return false;
    };
    let mut reader = MultiGzDecoder::new(file);
    let mut digest = Sha256::new();
    let mut length = 0;
    let mut buffer = [0; 65536];
    loop {
        let Ok(count) = reader.read(&mut buffer) else {
            return false;
        };
        if count == 0 {
            break;
        }
        length += count as u64;
        if length > metadata.raw_bytes {
            return false;
        }
        digest.update(&buffer[..count]);
    }
    length == metadata.raw_bytes && format!("{:x}", digest.finalize()) == metadata.raw_sha256
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
