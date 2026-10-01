mod common;

use common::*;
use serde_json::{Value, json};
use std::fs;

fn records(values: &[Value]) -> Vec<u8> {
    values
        .iter()
        .map(|v| format!("{v}\n"))
        .collect::<String>()
        .into_bytes()
}

#[test]
fn indexed_title_updates_only_sidecar_and_preserves_malformed_display_core() {
    let fixture = Fixture::new();
    let bytes = records(&[
        json!({"type":"session_meta","payload":{"id":"one","timestamp":"2026-01-01T00:00:00Z","cwd":"/work/project","source":"vscode","forked_from_id":"parent"}}),
        json!({"type":"response_item","timestamp":"2026-01-02T00:00:00Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"  Build\n this feature  "}]}}),
        json!({"type":"response_item","timestamp":"2026-01-03T00:00:00Z","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Done"}]}}),
        json!({"type":"turn_context","timestamp":"2026-01-09T00:00:00Z","payload":{}}),
    ]);
    let source = fixture.write("codex/sessions/one.jsonl", &bytes);
    fixture.write(
        "codex/session_index.jsonl",
        records(&[
            json!({"id":"one","thread_name":"First title","updated_at":"2026-01-04T00:00:00Z"}),
            json!({"id":"one","thread_name":"Older title","updated_at":"2026-01-03T00:00:00Z"}),
            json!({"id":"one","thread_name":"Invalid date","updated_at":"tomorrow"}),
        ]),
    );
    let report = fixture.collect(true);
    let path = archive(&report, 0);
    assert_eq!(raw(&path), bytes);
    assert_eq!(
        metadata(&path)["display"],
        json!({
            "version":1,"title":"First title","first_prompt":"Build this feature","cwd":"/work/project",
            "first_message_at":1767312000000u64,"last_message_at":1767398400000u64,
            "version_created_at":1767225600000u64,"source_kind":"conversation"
        })
    );
    let gzip = fs::read(&path).unwrap();
    let gzip_time = fs::metadata(&path).unwrap().modified().unwrap();
    let source_time = fs::metadata(&source).unwrap().modified().unwrap();
    let mut previous = metadata(&path);
    previous["display"] = json!({"version":{},"title":42});
    fs::write(sidecar(&path), serde_json::to_vec(&previous).unwrap()).unwrap();
    fixture.write("codex/session_index.jsonl", records(&[
        json!({"id":"one","thread_name":"Changed title","updated_at":"2026-01-04T00:00:00Z"}),
        json!({"id":"one","thread_name":"Latest equal-time title","updated_at":"2026-01-04T00:00:00Z"}),
    ]));
    assert_eq!(fixture.collect(true)["updated"], 1);
    assert_eq!(
        metadata(&path)["display"]["title"],
        "Latest equal-time title"
    );
    assert_eq!(fs::read(&path).unwrap(), gzip);
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), gzip_time);
    assert_eq!(fs::read(&source).unwrap(), bytes);
    assert_eq!(
        fs::metadata(&source).unwrap().modified().unwrap(),
        source_time
    );
    assert_eq!(fixture.collect(true)["unchanged"], 1);
}

#[test]
fn claude_titles_follow_record_order_and_skip_injected_or_tool_only_prompts() {
    let fixture = Fixture::new();
    fixture.write("claude/projects/project/one.jsonl", records(&[
        json!({"type":"user","sessionId":"one","timestamp":"2025-12-01T00:00:00Z","isMeta":true,"message":{"content":"Injected instructions"}}),
        json!({"type":"user","sessionId":"one","timestamp":"2025-12-01T00:00:01Z","isCompactSummary":true,"message":{"content":"A previous conversation summary"}}),
        json!({"type":"user","sessionId":"one","timestamp":"2025-12-02T00:00:00Z","message":{"content":[{"type":"tool_result","content":"tool output"}]}}),
        json!({"type":"user","sessionId":"one","timestamp":"2025-12-03T00:00:00Z","message":{"content":"# AGENTS.md instructions for /work\n<INSTRUCTIONS>rules</INSTRUCTIONS>"}}),
        json!({"type":"user","sessionId":"one","timestamp":"2025-12-04T00:00:00Z","message":{"content":"<environment_context>workspace</environment_context>"}}),
        json!({"type":"user","sessionId":"one","cwd":"/work","timestamp":"2026-01-02T02:00:00+02:00","message":{"content":"<system-reminder>ignore this</system-reminder> Please improve the picker"}}),
        json!({"type":"assistant","sessionId":"one","timestamp":"2026-01-03T00:00:00.123456Z","message":{"content":[{"type":"text","text":"Done"}]}}),
        json!({"type":"ai-title","sessionId":"one","aiTitle":"AI first"}),
        json!({"type":"custom-title","sessionId":"one","customTitle":"Custom first"}),
        json!({"type":"custom-title","sessionId":"one","customTitle":" Custom\n latest "}),
        json!({"type":"ai-title","sessionId":"one","aiTitle":"AI latest"}),
    ]));
    let report = fixture.collect(true);
    assert_eq!(
        metadata(&archive(&report, 0))["display"],
        json!({
            "version":1,"title":"Custom latest","first_prompt":"Please improve the picker","cwd":"/work",
            "first_message_at":1767312000000u64,"last_message_at":1767398400123u64,
            "version_created_at":1767312000000u64,"source_kind":"conversation"
        })
    );
}

#[test]
fn optional_index_failures_and_missing_metadata_do_not_fail_capture() {
    let fixture = Fixture::new();
    fixture.write("codex/sessions/one.jsonl", records(&[
        json!({"type":"session_meta","payload":{"id":"one","timestamp":"not-a-date","parent_thread_id":"other"}}),
        json!({"type":"event_msg","timestamp":"1969-12-31T23:59:59.9999Z","payload":{"type":"user_message","message":"Useful prompt"}}),
    ]));
    for content in [
        b"invalid\n".as_slice(),
        b"{\"id\":\"one\",\"thread_name\":true}\n",
    ] {
        fixture.write("codex/session_index.jsonl", content);
        let report = fixture.collect(true);
        assert_eq!(
            metadata(&archive(&report, 0))["display"],
            json!({
                "version":1,"first_prompt":"Useful prompt","source_kind":"unknown"
            })
        );
    }
    fs::remove_file(fixture.path("codex/session_index.jsonl")).unwrap();
    fs::File::create(fixture.path("codex/session_index.jsonl"))
        .unwrap()
        .set_len(8 * 1024 * 1024 + 1)
        .unwrap();
    assert_eq!(fixture.collect(true)["errors"], 0);
    fs::remove_file(fixture.path("codex/session_index.jsonl")).unwrap();
    fs::create_dir(fixture.path("codex/session_index.jsonl")).unwrap();
    fixture.write(
        "codex/sessions/legacy.jsonl",
        codex("legacy", "legacy event"),
    );
    assert_eq!(fixture.collect(true)["errors"], 0);
}

#[test]
fn explicit_subagent_signals_do_not_hide_forks() {
    let fixture = Fixture::new();
    for (id, payload) in [
        (
            "fork",
            json!({"id":"fork","source":"vscode","parent_thread_id":"parent","forked_from_id":"parent"}),
        ),
        (
            "sub",
            json!({"id":"sub","source":"vscode","thread_source":"subagent"}),
        ),
        (
            "spawn",
            json!({"id":"spawn","source":{"subagent":{"thread_spawn":{}}}}),
        ),
    ] {
        fixture.write(
            &format!("codex/sessions/{id}.jsonl"),
            records(&[json!({"type":"session_meta","payload":payload})]),
        );
    }
    fixture.write(
        "claude/projects/project/one/subagents/agent-abc.jsonl",
        records(&[json!({"type":"assistant","sessionId":"one","agentId":"abc"})]),
    );
    let report = fixture.collect(true);
    for i in 0..4 {
        let meta = metadata(&archive(&report, i));
        assert_eq!(
            meta["display"]["source_kind"],
            if meta["conversation_id"] == "fork" {
                "conversation"
            } else {
                "subagent"
            }
        );
    }
}

#[test]
fn metadata_text_is_single_line_and_unicode_bounded() {
    let fixture = Fixture::new();
    fixture.write("claude/projects/project/one.jsonl", records(&[
        json!({"type":"user","sessionId":"one","cwd":"p".repeat(5000),"message":{"content":"🎉".repeat(300)}}),
        json!({"type":"ai-title","sessionId":"one","aiTitle":"🎉".repeat(220)}),
    ]));
    let report = fixture.collect(true);
    let display = metadata(&archive(&report, 0))["display"].clone();
    assert_eq!(display["title"].as_str().unwrap().chars().count(), 200);
    assert_eq!(
        display["first_prompt"].as_str().unwrap().chars().count(),
        280
    );
    assert_eq!(display["cwd"].as_str().unwrap().chars().count(), 4096);
}

#[test]
fn current_item_events_skip_wrappers_and_count_only_message_prose() {
    let fixture = Fixture::new();
    fixture.write("codex/sessions/one.jsonl", records(&[
        json!({"type":"session_meta","payload":{"id":"one","source":"vscode"}}),
        json!({"type":"event_msg","timestamp":"2025-12-01T00:00:00Z","payload":{"type":"item_completed","item":{"type":"UserMessage","content":[{"type":"Text","text":"<skill>injected skill</skill>"}]}}}),
        json!({"type":"event_msg","timestamp":"2026-01-02T00:00:00Z","payload":{"type":"item_completed","item":{"type":"UserMessage","content":[{"type":"Text","text":"<SYSTEM-REMINDER source=\"injected\">context</SYSTEM-REMINDER> Add useful titles"}]}}}),
        json!({"type":"event_msg","timestamp":"2026-01-03T00:00:00Z","payload":{"type":"item_completed","item":{"type":"AgentMessage","content":[{"type":"Text","text":"Added titles"}]}}}),
        json!({"type":"response_item","timestamp":"2026-01-04T00:00:00Z","payload":{"type":"message","role":"assistant","content":[{"type":"tool_use","name":"write_file"}]}}),
    ]));
    let report = fixture.collect(true);
    assert_eq!(
        metadata(&archive(&report, 0))["display"],
        json!({
            "version":1,"first_prompt":"Add useful titles","source_kind":"conversation",
            "first_message_at":1767312000000u64,"last_message_at":1767398400000u64
        })
    );
}

#[test]
fn malformed_old_display_does_not_lose_preserved_attachment_mappings() {
    let fixture = Fixture::new();
    let image = fixture.write("media/image.png", b"image");
    fixture.write("codex/sessions/one.jsonl", records(&[
        json!({"type":"session_meta","payload":{"id":"one"}}),
        json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_image","image_url":image}]}}),
    ]));
    let report = fixture.collect(true);
    let path = archive(&report, 0);
    let mut previous = metadata(&path);
    let mappings = previous["attachments"].clone();
    assert_eq!(mappings[0]["status"], "copied");
    previous["display"] = json!(false);
    fs::write(sidecar(&path), serde_json::to_vec(&previous).unwrap()).unwrap();
    fs::remove_file(image).unwrap();
    assert_eq!(fixture.collect(true)["updated"], 1);
    assert_eq!(metadata(&path)["attachments"], mappings);
}

#[cfg(unix)]
#[test]
fn optional_title_index_never_follows_symlinks() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    fixture.write("codex/sessions/one.jsonl", codex("one", "hello"));
    let target = fixture.write(
        "outside/index.jsonl",
        records(&[
            json!({"id":"one","thread_name":"Outside title","updated_at":"2026-01-04T00:00:00Z"}),
        ]),
    );
    symlink(target, fixture.path("codex/session_index.jsonl")).unwrap();
    let report = fixture.collect(true);
    assert!(
        metadata(&archive(&report, 0))["display"]
            .get("title")
            .is_none()
    );
}

#[test]
fn explicit_subagent_and_first_native_context_survive_later_session_metadata() {
    let fixture = Fixture::new();
    fixture.write("codex/sessions/one.jsonl", records(&[
        json!({"type":"session_meta","payload":{"id":"one","thread_source":"subagent","cwd":"/original","timestamp":"2026-01-01T00:00:00Z"}}),
        json!({"type":"session_meta","payload":{"id":"one","source":"vscode","cwd":"/later","timestamp":"2026-02-01T00:00:00Z"}}),
        json!({"type":"session_meta","payload":{"id":"one"}}),
    ]));
    let report = fixture.collect(true);
    assert_eq!(
        metadata(&archive(&report, 0))["display"],
        json!({
            "version":1,"source_kind":"subagent","cwd":"/original","version_created_at":1767225600000u64
        })
    );
}

#[test]
fn display_limit_never_discards_attachment_mappings_or_rewrites_gzip() {
    let fixture = Fixture::new();
    let image = fixture.write("media/image.png", b"image");
    let native = |id: &str| {
        records(&[
            json!({"type":"session_meta","payload":{"id":id,"source":"vscode"}}),
            json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_image","image_url":image}]}}),
        ])
    };
    fixture.write("codex/sessions/one.jsonl", native("one"));
    let initial = fixture.collect(true);
    let path = archive(&initial, 0);
    let padding = 1_999_600 - fs::metadata(sidecar(&path)).unwrap().len() as usize;
    let id = "a".repeat(padding + 3);
    let bytes = native(&id);
    let source = fixture.write("codex/sessions/one.jsonl", &bytes);
    fixture.collect(true);
    let before = metadata(&path);
    assert!(before.get("display").is_some());
    assert_eq!(before["attachments"][0]["status"], "copied");
    let gzip = fs::read(&path).unwrap();
    let gzip_time = fs::metadata(&path).unwrap().modified().unwrap();
    let source_time = fs::metadata(&source).unwrap().modified().unwrap();
    fs::remove_file(image).unwrap();
    fixture.write(
        "codex/session_index.jsonl",
        records(&[
            json!({"id":id,"thread_name":"🎉".repeat(200),"updated_at":"2026-01-04T00:00:00Z"}),
        ]),
    );
    let report = fixture.collect(true);
    assert_eq!(report["updated"], 1);
    assert_eq!(report["omissions"], 0);
    let after = metadata(&path);
    assert!(after.get("display").is_none());
    assert_eq!(after["attachments"], before["attachments"]);
    assert!(fs::metadata(sidecar(&path)).unwrap().len() <= 2_000_000);
    assert_eq!(fs::read(&path).unwrap(), gzip);
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), gzip_time);
    assert_eq!(fs::read(&source).unwrap(), bytes);
    assert_eq!(
        fs::metadata(&source).unwrap().modified().unwrap(),
        source_time
    );
    assert_eq!(fixture.collect(true)["unchanged"], 1);
}
