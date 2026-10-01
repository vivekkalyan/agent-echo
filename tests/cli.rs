mod common;

use common::*;
use serde_json::{Value, json};
use std::{fs, process::Command};

#[test]
fn preserves_every_byte_and_is_idempotent() {
    let fixture = Fixture::new();
    let bytes = b"{ \"type\":\"session_meta\", \"payload\":{\"id\":\"one\"}}\r\n{\"type\":\"future_record\",\"unknown\":[1,2]}\n{\"type\":\"event_msg\",\"payload\":{\"message\":\"hello\"}}";
    let source = fixture.write("codex/sessions/2026/rollout-one.jsonl", bytes);
    let source_time = fs::metadata(&source).unwrap().modified().unwrap();
    let first = fixture.collect(true);
    assert_eq!(first["updated"], 1);
    let path = archive(&first, 0);
    assert_eq!(raw(&path), bytes);
    assert_eq!(fs::read(&path).unwrap()[4..8], [0, 0, 0, 0]);
    let gzip_time = fs::metadata(&path).unwrap().modified().unwrap();
    let meta_time = fs::metadata(sidecar(&path)).unwrap().modified().unwrap();
    let second = fixture.collect(true);
    assert_eq!(second["unchanged"], 1);
    assert_eq!(second["updated"], 0);
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), gzip_time);
    assert_eq!(
        fs::metadata(sidecar(&path)).unwrap().modified().unwrap(),
        meta_time
    );
    assert_eq!(
        fs::metadata(source).unwrap().modified().unwrap(),
        source_time
    );
    let status: Value = serde_json::from_slice(&fixture.run("status").stdout).unwrap();
    assert_eq!(status, second);
}

#[test]
fn codex_move_preserves_identity_and_pages_remain_distinct() {
    let fixture = Fixture::new();
    let source = fixture.write(
        "codex/sessions/day/rollout-one.jsonl",
        codex("same-conversation", "page one"),
    );
    fixture.write(
        "codex/sessions/day/rollout-two.jsonl",
        codex("same-conversation", "page two"),
    );
    let first = fixture.collect(true);
    assert_eq!(first["updated"], 2);
    let path = archive(&first, 0);
    assert_eq!(path.parent(), archive(&first, 1).parent());
    fs::create_dir_all(fixture.path("codex/archived_sessions")).unwrap();
    fs::rename(
        source,
        fixture.path("codex/archived_sessions/rollout-one.jsonl"),
    )
    .unwrap();
    let second = fixture.collect(true);
    assert_eq!(archive(&second, 0), path);
    assert_eq!(raw(&path), codex("same-conversation", "page one"));
    assert!(
        metadata(&path)["source_path"]
            .as_str()
            .unwrap()
            .contains("archived_sessions")
    );
}

#[test]
fn claude_main_subagents_and_superseded_files_survive() {
    let fixture = Fixture::new();
    fixture.write(
        "claude/projects/project/session.jsonl",
        b"{\"type\":\"user\",\"sessionId\":\"session\"}\n",
    );
    fixture.write(
        "claude/projects/project/session/subagents/agent-abc.jsonl",
        b"{\"type\":\"assistant\",\"sessionId\":\"session\",\"agentId\":\"abc\"}\n",
    );
    fixture.write(
        "claude/projects/project/session.jsonl.superseded-123",
        b"{\"type\":\"user\",\"sessionId\":\"session\",\"old\":true}\n",
    );
    let report = fixture.collect(true);
    assert_eq!(report["updated"], 3);
    let metas: Vec<_> = (0..3).map(|i| metadata(&archive(&report, i))).collect();
    assert_eq!(
        metas
            .iter()
            .filter(|m| m["conversation_id"] == "session")
            .count(),
        2
    );
    let child = metas
        .iter()
        .find(|m| m["parent_conversation_id"] == "session")
        .unwrap();
    assert_eq!(child["conversation_id"], "session:abc");
}

#[test]
fn same_size_rewrite_refreshes_current_and_deletion_retains_it() {
    let fixture = Fixture::new();
    let source = fixture.write("codex/sessions/one.jsonl", codex("one", "AAAA"));
    let first = fixture.collect(true);
    let path = archive(&first, 0);
    fixture.write("codex/sessions/one.jsonl", codex("one", "BBBB"));
    let second = fixture.collect(true);
    assert_eq!(second["updated"], 1);
    assert_eq!(archive(&second, 0), path);
    assert_eq!(raw(&path), codex("one", "BBBB"));
    fs::remove_file(source).unwrap();
    assert_eq!(fixture.collect(true)["discovered"], 0);
    assert_eq!(raw(&path), codex("one", "BBBB"));
}

#[test]
fn partial_record_fails_without_replacing_previous_and_other_sources_continue() {
    let fixture = Fixture::new();
    fixture.write("codex/sessions/one.jsonl", codex("one", "complete"));
    let first = fixture.collect(true);
    let path = archive(&first, 0);
    let original = fs::read(&path).unwrap();
    let original_meta = fs::read(sidecar(&path)).unwrap();
    let mut partial = codex("one", "complete");
    partial.extend_from_slice(b"{\"type\":");
    fixture.write("codex/sessions/one.jsonl", partial);
    fixture.write("codex/sessions/two.jsonl", codex("two", "complete too"));
    let report = fixture.collect(false);
    assert_eq!(report["errors"], 1);
    assert_eq!(report["updated"], 1);
    assert_eq!(fs::read(path).unwrap(), original);
    assert_eq!(
        fs::read(sidecar(&archive(&first, 0))).unwrap(),
        original_meta
    );
}

#[test]
fn malformed_interior_records_are_preserved_with_diagnostics() {
    let fixture = Fixture::new();
    let bytes = b"{\"type\":\"session_meta\",\"payload\":{\"id\":\"one\"}}\nnot json at all\n{\"type\":\"event_msg\"}\n";
    fixture.write("codex/sessions/one.jsonl", bytes);
    let report = fixture.collect(true);
    assert_eq!(raw(&archive(&report, 0)), bytes);
    assert_eq!(
        report["sources"][0]["diagnostics"][0],
        "malformed interior JSON preserved at line 2"
    );
}

#[test]
fn compressed_cap_failure_keeps_previous_snapshot() {
    let fixture = Fixture::new();
    fixture.write("codex/sessions/one.jsonl", codex("one", "complete"));
    let first = fixture.collect(true);
    let path = archive(&first, 0);
    let before = fs::read(&path).unwrap();
    fixture.config(20_000_000, 20);
    fixture.write("codex/sessions/one.jsonl", codex("one", "a replacement"));
    let report = fixture.collect(false);
    assert_eq!(report["errors"], 1);
    assert!(
        report["sources"][0]["error"]
            .as_str()
            .unwrap()
            .contains("exceeding")
    );
    assert_eq!(fs::read(path).unwrap(), before);
}

#[test]
fn differing_duplicate_sources_conflict_identical_duplicates_converge() {
    let fixture = Fixture::new();
    fixture.write("codex/sessions/one.jsonl", codex("one", "A"));
    let initial = fixture.collect(true);
    let path = archive(&initial, 0);
    fixture.write("codex/archived_sessions/one.jsonl", codex("one", "B"));
    let conflict = fixture.collect(false);
    assert_eq!(conflict["errors"], 1);
    assert!(
        conflict["sources"][0]["error"]
            .as_str()
            .unwrap()
            .contains("conflicting live files")
    );
    assert_eq!(raw(&path), codex("one", "A"));
    fixture.write("codex/archived_sessions/one.jsonl", codex("one", "A"));
    assert_eq!(fixture.collect(true)["errors"], 0);
    assert_eq!(raw(&path), codex("one", "A"));
}

#[test]
fn missing_and_stale_sidecars_are_repaired_without_rewriting_gzip() {
    let fixture = Fixture::new();
    fixture.write("codex/sessions/one.jsonl", codex("one", "A"));
    let initial = fixture.collect(true);
    let path = archive(&initial, 0);
    let before = fs::metadata(&path).unwrap().modified().unwrap();
    fs::remove_file(sidecar(&path)).unwrap();
    assert_eq!(fixture.collect(true)["updated"], 1);
    assert_eq!(metadata(&path)["raw_bytes"], codex("one", "A").len());
    let mut stale = metadata(&path);
    stale["raw_sha256"] = json!("f".repeat(64));
    fs::write(sidecar(&path), serde_json::to_vec(&stale).unwrap()).unwrap();
    assert_eq!(fixture.collect(true)["updated"], 1);
    assert_ne!(metadata(&path)["raw_sha256"], stale["raw_sha256"]);
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), before);
}

#[test]
fn overlapping_config_fails_before_any_writes() {
    let fixture = Fixture::new();
    let config = fs::read_to_string(fixture.path("config.toml"))
        .unwrap()
        .replace(
            fixture.path("archive").to_str().unwrap(),
            fixture.path("codex/archive").to_str().unwrap(),
        );
    fs::write(fixture.path("config.toml"), config).unwrap();
    let output = fixture.run("collect");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("overlap"));
    assert!(!fixture.path("state").exists());
    let good = Fixture::new();
    assert!(good.run("check-config").status.success());
    assert!(!good.path("state").exists());
}

#[cfg(unix)]
#[test]
fn symlink_config_is_rejected_before_writes() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.path("outside")).unwrap();
    symlink(fixture.path("outside"), fixture.path("archive")).unwrap();
    let output = fixture.run("collect");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("symlink"));
    assert!(!fixture.path("state").exists());
}

#[test]
fn producer_ids_isolate_the_same_native_source() {
    let fixture = Fixture::new();
    fixture.write("codex/sessions/one.jsonl", codex("one", "A"));
    let first = fixture.collect(true);
    let config = fs::read_to_string(fixture.path("config.toml"))
        .unwrap()
        .replace("test-machine", "other-machine");
    fs::write(fixture.path("config.toml"), config).unwrap();
    let second = fixture.collect(true);
    assert_eq!(second["updated"], 1);
    assert_ne!(archive(&first, 0), archive(&second, 0));
    assert_eq!(metadata(&archive(&second, 0))["producer"], "other-machine");
    assert_eq!(raw(&archive(&first, 0)), raw(&archive(&second, 0)));
}

#[test]
fn local_lock_excludes_concurrent_collection_and_releases_after_exit() {
    let fixture = Fixture::new();
    fixture.write("codex/sessions/one.jsonl", codex("one", "complete"));
    fixture.collect(true);
    let lock_path = fs::read_dir(fixture.path("state"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension().unwrap() == "lock")
        .unwrap();
    let lock = fs::File::open(lock_path).unwrap();
    lock.lock().unwrap();
    let output = fixture.run("collect");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("another collection"));
    drop(lock);
    assert_eq!(fixture.collect(true)["unchanged"], 1);
}

#[cfg(unix)]
#[test]
fn symlinked_system_parent_is_resolved_at_config_boundary() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.path("physical")).unwrap();
    symlink(fixture.path("physical"), fixture.path("system-alias")).unwrap();
    let config = fs::read_to_string(fixture.path("config.toml"))
        .unwrap()
        .replace(
            fixture.path("archive").to_str().unwrap(),
            fixture.path("system-alias/archive").to_str().unwrap(),
        );
    fs::write(fixture.path("config.toml"), config).unwrap();
    fixture.write("codex/sessions/one.jsonl", codex("one", "A"));
    let report = fixture.collect(true);
    assert!(
        archive(&report, 0)
            .starts_with(fs::canonicalize(fixture.path("physical/archive")).unwrap())
    );
    assert_eq!(raw(&archive(&report, 0)), codex("one", "A"));
}

#[test]
fn corrupted_archive_is_repaired_even_when_native_source_is_unchanged() {
    let fixture = Fixture::new();
    fixture.write("codex/sessions/one.jsonl", codex("one", "A"));
    let first = fixture.collect(true);
    let path = archive(&first, 0);
    let original = fs::read(&path).unwrap();
    let mut corrupt = original.clone();
    corrupt.extend_from_slice(b"trailing garbage");
    fs::write(&path, corrupt).unwrap();
    assert_eq!(fixture.collect(true)["updated"], 1);
    assert_eq!(fs::read(&path).unwrap(), original);
    assert_eq!(raw(&path), codex("one", "A"));
}

#[test]
fn oversized_base_metadata_keeps_old_sidecar_but_publishes_complete_native_bytes() {
    let fixture = Fixture::new();
    fixture.write("codex/sessions/one.jsonl", codex("one", "before"));
    let first = fixture.collect(true);
    let path = archive(&first, 0);
    let old_sidecar = fs::read(sidecar(&path)).unwrap();
    let native = format!(
        "{}\n",
        json!({"type":"session_meta","payload":{"id":"x".repeat(2_000_000)}})
    );
    fixture.write("codex/sessions/one.jsonl", &native);
    let report = fixture.collect(true);
    assert_eq!(report["updated"], 1);
    assert_eq!(report["errors"], 0);
    assert!(
        report["sources"][0]["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d.as_str().unwrap().contains("sidecar publication skipped"))
    );
    assert_eq!(fs::read(sidecar(&path)).unwrap(), old_sidecar);
    assert_eq!(raw(&path), native.as_bytes());
}

#[test]
fn claude_discovery_keeps_documented_set_aside_names_and_ignores_temporary_files() {
    let fixture = Fixture::new();
    let bytes =
        b"{\"type\":\"user\",\"sessionId\":\"session\",\"message\":{\"content\":\"hello\"}}\n";
    for name in [
        "session.jsonl",
        "session.jsonl.superseded-1750000000000",
        "session.orphaned-1750000000000-abcdef.jsonl",
    ] {
        fixture.write(&format!("claude/projects/project/{name}"), bytes);
    }
    for name in [
        "session.jsonl.gz",
        "session.jsonl.lock",
        "session.jsonl.tmp",
        "session.jsonl.123456",
        "session.jsonl.bak",
        "session.jsonl.superseded-",
        "session.jsonl.superseded-123.tmp",
        "session.jsonl.superseded-nope",
        "session.orphaned-nope-abc.jsonl",
        "session.orphaned-123-.jsonl",
        "session.jsonl.tmp.jsonl",
    ] {
        fixture.write(
            &format!("claude/projects/project/{name}"),
            b"not a transcript",
        );
    }
    let report = fixture.collect(true);
    assert_eq!(report["discovered"], 3);
    assert_eq!(report["updated"], 3);
    for index in 0..3 {
        assert_eq!(raw(&archive(&report, index)), bytes);
    }
}

#[cfg(unix)]
#[test]
fn archive_discovery_failure_replaces_previous_healthy_status() {
    use std::os::unix::{fs::PermissionsExt, process::CommandExt};
    let fixture = Fixture::new();
    fixture.write("codex/sessions/one.jsonl", codex("one", "complete"));
    fixture.collect(true);
    fs::create_dir(fixture.path("archive/unreadable")).unwrap();
    fs::set_permissions(fixture.root.path(), fs::Permissions::from_mode(0o755)).unwrap();
    fs::set_permissions(fixture.path("archive"), fs::Permissions::from_mode(0o755)).unwrap();
    fs::set_permissions(fixture.path("state"), fs::Permissions::from_mode(0o777)).unwrap();
    for entry in fs::read_dir(fixture.path("state")).unwrap() {
        fs::set_permissions(entry.unwrap().path(), fs::Permissions::from_mode(0o666)).unwrap();
    }
    fs::set_permissions(
        fixture.path("archive/unreadable"),
        fs::Permissions::from_mode(0o000),
    )
    .unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_agent-echo"));
    command
        .arg("--config")
        .arg(fixture.path("config.toml"))
        .arg("collect");
    if Command::new("id").arg("-u").output().unwrap().stdout == b"0\n" {
        command.gid(65534).uid(65534);
    }
    let output = command.output().unwrap();
    fs::set_permissions(
        fixture.path("archive/unreadable"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    assert!(!output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|_| panic!("{}", String::from_utf8_lossy(&output.stderr)));
    assert_eq!(report["errors"], 1);
    assert!(
        report["fatal"]
            .as_str()
            .unwrap()
            .contains("archive discovery failed")
    );
    let status: Value = serde_json::from_slice(&fixture.run("status").stdout).unwrap();
    assert_eq!(status, report);
}

#[test]
fn config_is_required_by_cli_parser() {
    let output = Command::new(env!("CARGO_BIN_EXE_agent-echo"))
        .arg("collect")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--config <CONFIG>"));
    let fixture = Fixture::new();
    assert!(fixture.run("check-config").status.success());
}

#[test]
fn status_reads_reports_written_before_attachment_support() {
    let fixture = Fixture::new();
    fixture.write("codex/sessions/one.jsonl", codex("one", "complete"));
    let expected = fixture.collect(true);
    let mut legacy = expected.clone();
    legacy.as_object_mut().unwrap().remove("omissions");
    for source in legacy["sources"].as_array_mut().unwrap() {
        source.as_object_mut().unwrap().remove("omissions");
    }
    let report_path = fs::read_dir(fixture.path("state"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension().unwrap() == "json")
        .unwrap();
    let bytes = serde_json::to_vec(&legacy).unwrap();
    fs::write(&report_path, &bytes).unwrap();
    let output = fixture.run("status");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let status: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(status, expected);
    assert_eq!(fs::read(report_path).unwrap(), bytes);
}
