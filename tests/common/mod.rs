use flate2::read::GzDecoder;
use serde_json::Value;
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Output},
};
use tempfile::TempDir;

pub struct Fixture {
    pub root: TempDir,
}
impl Fixture {
    pub fn new() -> Self {
        let fixture = Self {
            root: tempfile::tempdir().unwrap(),
        };
        fixture.config(200_000_000);
        fixture
    }
    pub fn path(&self, relative: &str) -> PathBuf {
        self.root.path().join(relative)
    }
    pub fn config(&self, transcript_cap: u64) {
        let config = format!(
            "archive_root = {:?}\nstate_dir = {:?}\nproducer = \"test-machine\"\ncodex_home = {:?}\nclaude_home = {:?}\nmax_compressed_transcript_bytes = {transcript_cap}\n",
            self.path("archive"),
            self.path("state"),
            self.path("codex"),
            self.path("claude")
        );
        fs::write(self.path("config.toml"), config).unwrap();
    }
    pub fn write(&self, relative: &str, bytes: impl AsRef<[u8]>) -> PathBuf {
        let path = self.path(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        path
    }
    pub fn run(&self, command: &str) -> Output {
        Command::new(env!("CARGO_BIN_EXE_agent-echo"))
            .arg("--config")
            .arg(self.path("config.toml"))
            .arg(command)
            .output()
            .unwrap()
    }
    pub fn collect(&self, success: bool) -> Value {
        let output = self.run("collect");
        assert_eq!(
            output.status.success(),
            success,
            "stderr={} stdout={}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
}

pub fn codex(id: &str, message: &str) -> Vec<u8> {
    format!("{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{id}\"}}}}\n{{\"type\":\"event_msg\",\"payload\":{{\"message\":\"{message}\"}}}}\n").into_bytes()
}
pub fn archive(report: &Value, index: usize) -> PathBuf {
    PathBuf::from(report["sources"][index]["archive"].as_str().unwrap())
}
pub fn raw(path: &Path) -> Vec<u8> {
    let mut bytes = Vec::new();
    GzDecoder::new(fs::File::open(path).unwrap())
        .read_to_end(&mut bytes)
        .unwrap();
    bytes
}
pub fn sidecar(path: &Path) -> PathBuf {
    PathBuf::from(path.to_str().unwrap().replace(".jsonl.gz", ".meta.json"))
}
pub fn metadata(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(sidecar(path)).unwrap()).unwrap()
}
