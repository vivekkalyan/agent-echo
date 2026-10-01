use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    env, fs,
    path::{Component, Path, PathBuf},
};

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub archive_root: PathBuf,
    pub state_dir: PathBuf,
    pub producer: String,
    #[serde(default = "codex_home")]
    pub codex_home: PathBuf,
    #[serde(default = "claude_home")]
    pub claude_home: PathBuf,
    #[serde(default)]
    pub attachment_roots: Option<Vec<PathBuf>>,
    #[serde(default = "attachment_cap")]
    pub max_attachment_bytes: u64,
    #[serde(default = "transcript_cap")]
    pub max_compressed_transcript_bytes: u64,
}

fn home() -> PathBuf {
    env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}
fn codex_home() -> PathBuf {
    env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".codex"))
}
fn claude_home() -> PathBuf {
    env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".claude"))
}
fn attachment_cap() -> u64 {
    20_000_000
}
fn transcript_cap() -> u64 {
    200_000_000
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let mut config: Self = toml::from_str(
            &fs::read_to_string(path).with_context(|| format!("read config {}", path.display()))?,
        )?;
        if config.producer.trim().is_empty() || config.producer.len() > 200 {
            bail!("producer must contain 1 to 200 bytes and identify this machine");
        }
        if config.max_attachment_bytes == 0 || config.max_compressed_transcript_bytes == 0 {
            bail!("size caps must be positive");
        }
        config.archive_root = absolute(&config.archive_root)?;
        config.state_dir = absolute(&config.state_dir)?;
        config.codex_home = absolute(&config.codex_home)?;
        config.claude_home = absolute(&config.claude_home)?;
        let roots = config
            .attachment_roots
            .take()
            .unwrap_or_else(|| vec![config.codex_home.join("attachments")]);
        config.attachment_roots = Some(roots.iter().map(|p| absolute(p)).collect::<Result<_>>()?);
        let directories = [
            &config.archive_root,
            &config.state_dir,
            &config.codex_home,
            &config.claude_home,
        ];
        for directory in directories {
            checked_path(directory)?;
            if directory.exists() && !directory.is_dir() {
                bail!("not a directory: {}", directory.display());
            }
        }
        for (index, left) in directories.iter().enumerate() {
            for right in &directories[index + 1..] {
                if overlap(left, right) {
                    bail!(
                        "configured paths overlap: {} and {}",
                        left.display(),
                        right.display()
                    );
                }
            }
        }
        for root in config.attachment_roots.as_ref().unwrap() {
            checked_path(root)?;
            if root.exists() && !root.is_dir() {
                bail!("attachment root is not a directory: {}", root.display());
            }
            if overlap(root, &config.archive_root) || overlap(root, &config.state_dir) {
                bail!("attachment roots must not overlap archive_root or state_dir");
            }
        }
        if fs::canonicalize(path)?.starts_with(&config.archive_root) {
            bail!("config must be outside archive_root");
        }
        Ok(config)
    }
}

fn absolute(path: &Path) -> Result<PathBuf> {
    let path = if path == Path::new("~") {
        home()
    } else if let Ok(tail) = path.strip_prefix("~/") {
        home().join(tail)
    } else {
        path.to_owned()
    };
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        bail!(
            "paths must be absolute without traversal: {}",
            path.display()
        );
    }
    if fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
        bail!(
            "configured directory must not be a symlink: {}",
            path.display()
        );
    }
    let mut ancestor = path.as_path();
    let mut suffix = Vec::new();
    while !ancestor.exists() {
        suffix.push(
            ancestor
                .file_name()
                .context("path has no existing ancestor")?,
        );
        ancestor = ancestor.parent().context("path has no existing ancestor")?;
    }
    let mut resolved = fs::canonicalize(ancestor)?;
    for component in suffix.into_iter().rev() {
        resolved.push(component);
    }
    Ok(resolved)
}

fn overlap(a: &Path, b: &Path) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

pub fn checked_path(path: &Path) -> Result<()> {
    let mut current = PathBuf::new();
    for part in path.components() {
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                bail!("symlink path is not permitted: {}", current.display())
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| format!("inspect {}", current.display()));
            }
        }
    }
    Ok(())
}
