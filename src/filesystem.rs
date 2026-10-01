use crate::config::checked_path;
use anyhow::{Context, Result, bail};
use rustix::fs::{self, Mode, OFlags};
use std::{
    fs::File,
    io::{Read, Write},
    path::{Component, Path},
    sync::atomic::{AtomicU64, Ordering},
};

fn directory(path: &Path, create: bool) -> Result<File> {
    let mut current = File::from(fs::open(
        "/",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    for component in path.components() {
        let name = match component {
            Component::RootDir => continue,
            Component::Normal(name) => name,
            _ => bail!("filesystem path must be absolute without traversal"),
        };
        let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW;
        let next = match fs::openat(&current, name, flags, Mode::empty()) {
            Ok(next) => next,
            Err(rustix::io::Errno::NOENT) if create => {
                match fs::mkdirat(&current, name, Mode::RUSR | Mode::WUSR | Mode::XUSR) {
                    Ok(()) | Err(rustix::io::Errno::EXIST) => {}
                    Err(error) => return Err(error.into()),
                }
                fs::openat(&current, name, flags, Mode::empty())?
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("open directory {} without symlinks", path.display())
                });
            }
        };
        current = File::from(next);
    }
    Ok(current)
}

pub fn open_regular(path: &Path) -> Result<File> {
    let parent = directory(path.parent().context("file has no parent")?, false)?;
    let name = path.file_name().context("file has no name")?;
    let file = File::from(fs::openat(
        parent,
        name,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::empty(),
    )?);
    if !file.metadata()?.is_file() {
        bail!("not a regular file: {}", path.display());
    }
    Ok(file)
}

pub fn open_lock(path: &Path) -> Result<File> {
    let parent = directory(path.parent().context("lock has no parent")?, true)?;
    let name = path.file_name().context("lock has no name")?;
    let file = File::from(fs::openat(
        parent,
        name,
        OFlags::RDWR | OFlags::CREATE | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::RUSR | Mode::WUSR,
    )?);
    if !file.metadata()?.is_file() {
        bail!("lock is not a regular file");
    }
    Ok(file)
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<bool> {
    checked_path(path)?;
    let parent = directory(path.parent().context("file has no parent")?, true)?;
    let name = path.file_name().context("file has no name")?;
    match fs::openat(
        &parent,
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(file) => {
            let mut file = File::from(file);
            let metadata = file.metadata()?;
            if !metadata.is_file() {
                bail!("destination is not a regular file: {}", path.display());
            }
            if metadata.len() == bytes.len() as u64 {
                let mut previous = Vec::new();
                (&mut file)
                    .take(bytes.len() as u64 + 1)
                    .read_to_end(&mut previous)?;
                if previous == bytes {
                    return Ok(false);
                }
            }
        }
        Err(rustix::io::Errno::NOENT) => {}
        Err(error) => return Err(error.into()),
    }
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let (temporary, mut file) = loop {
        let temporary = format!(
            ".agent-echo-{}-{}.tmp",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        );
        match fs::openat(
            &parent,
            &temporary,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        ) {
            Ok(file) => break (temporary, File::from(file)),
            Err(rustix::io::Errno::EXIST) => continue,
            Err(error) => return Err(error.into()),
        }
    };
    let result = (|| -> Result<()> {
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::renameat(&parent, &temporary, &parent, name)?;
        parent.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::unlinkat(&parent, &temporary, fs::AtFlags::empty());
    }
    result.with_context(|| format!("publish {}", path.display()))?;
    Ok(true)
}
