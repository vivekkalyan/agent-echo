mod common;

use common::*;
use serde_json::{Value, json};
use std::{fs, process::Command};

#[test]
fn attachments_are_copied_omitted_and_carried_forward() {
    let fixture = Fixture::new();
    fixture.config(5, 200_000_000);
    let good = fixture.write("media/good.png", b"image");
    let huge = fixture.write("media/huge.png", b"too big");
    let outside = fixture.write("outside/private.png", b"no");
    let transcript = format!(
        "{}\n{}\n",
        json!({"type":"session_meta","payload":{"id":"one"}}),
        json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_image","image_url":good},{"type":"input_image","image_url":huge},{"type":"input_image","image_url":outside},{"type":"text","text":format!("![missing]({})",fixture.path("media/missing.png").display())}]}})
    );
    fixture.write("codex/sessions/one.jsonl", transcript);
    let report = fixture.collect(true);
    assert_eq!(report["omissions"], 3);
    let path = archive(&report, 0);
    let meta = metadata(&path);
    let copied = meta["attachments"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["status"] == "copied")
        .unwrap();
    let attachment = path
        .parent()
        .unwrap()
        .join(copied["path"].as_str().unwrap());
    assert_eq!(fs::read(&attachment).unwrap(), b"image");
    let before = fs::metadata(&attachment).unwrap().modified().unwrap();
    fs::remove_file(good).unwrap();
    let again = fixture.collect(true);
    assert_eq!(again["unchanged"], 1);
    assert_eq!(metadata(&path), meta);
    assert_eq!(
        fs::metadata(&attachment).unwrap().modified().unwrap(),
        before
    );
    assert_eq!(fs::read(attachment).unwrap(), b"image");
}

#[test]
fn arbitrary_tool_paths_are_not_attachment_candidates() {
    let fixture = Fixture::new();
    let secret = fixture.write("media/secret.txt", b"secret");
    let transcript = format!(
        "{}\n{}\n",
        json!({"type":"session_meta","payload":{"id":"one"}}),
        json!({"type":"response_item","payload":{"type":"function_call","arguments":{"file_path":secret},"output":secret.to_str().unwrap()}})
    );
    fixture.write("codex/sessions/one.jsonl", transcript);
    let report = fixture.collect(true);
    assert_eq!(metadata(&archive(&report, 0))["attachments"], json!([]));
    assert_eq!(
        raw(&archive(&report, 0)),
        fs::read(fixture.path("codex/sessions/one.jsonl")).unwrap()
    );
}

#[cfg(unix)]
#[test]
fn symlink_escape_is_omitted_and_destination_symlinks_are_rejected() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    fixture.write("outside/private.png", b"secret");
    fs::create_dir_all(fixture.path("media")).unwrap();
    symlink(
        fixture.path("outside/private.png"),
        fixture.path("media/link.png"),
    )
    .unwrap();
    fixture.write(
        "codex/sessions/one.jsonl",
        format!(
            "{}\n{}\n",
            json!({"type":"session_meta","payload":{"id":"one"}}),
            json!({"type":"input_image","image_url":fixture.path("media/link.png")})
        ),
    );
    let first = fixture.collect(true);
    let path = archive(&first, 0);
    assert_eq!(
        metadata(&path)["attachments"][0]["reason"],
        "outside_allowed_roots"
    );
    fs::remove_file(&path).unwrap();
    symlink(fixture.path("outside/private.png"), &path).unwrap();
    assert_eq!(fixture.collect(false)["errors"], 1);
    assert_eq!(
        fs::read(fixture.path("outside/private.png")).unwrap(),
        b"secret"
    );
}

#[test]
fn defaults_use_native_homes_and_only_codex_attachment_root() {
    let fixture = Fixture::new();
    fs::write(
        fixture.path("config.toml"),
        format!(
            "archive_root = {:?}\nstate_dir = {:?}\nproducer = \"default-test\"\n",
            fixture.path("archive"),
            fixture.path("state")
        ),
    )
    .unwrap();
    let image = fixture.write("codex/attachments/report.html", b"<p>Report</p>");
    fixture.write("codex/sessions/one.jsonl", format!("{}\n{}\n", json!({"type":"session_meta","payload":{"id":"one"}}), json!({"type":"event_msg","payload":{"type":"agent_message","message":format!("[Report]({})",image.display())}})));
    let output = Command::new(env!("CARGO_BIN_EXE_agent-echo"))
        .arg("--config")
        .arg(fixture.path("config.toml"))
        .arg("collect")
        .env("CODEX_HOME", fixture.path("codex"))
        .env("CLAUDE_CONFIG_DIR", fixture.path("claude"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["updated"], 1);
    let path = archive(&report, 0);
    let meta = metadata(&path);
    assert_eq!(meta["attachments"][0]["status"], "copied");
    assert_eq!(
        fs::read(
            path.parent()
                .unwrap()
                .join(meta["attachments"][0]["path"].as_str().unwrap())
        )
        .unwrap(),
        b"<p>Report</p>"
    );
}

#[cfg(unix)]
#[test]
fn fifo_attachment_is_omitted_without_blocking() {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.path("media")).unwrap();
    assert!(
        Command::new("mkfifo")
            .arg(fixture.path("media/pipe.png"))
            .status()
            .unwrap()
            .success()
    );
    fixture.write(
        "codex/sessions/one.jsonl",
        format!(
            "{}\n{}\n",
            json!({"type":"session_meta","payload":{"id":"one"}}),
            json!({"type":"input_image","image_url":fixture.path("media/pipe.png")})
        ),
    );
    let report = fixture.collect(true);
    assert_eq!(report["omissions"], 1);
    assert_eq!(
        metadata(&archive(&report, 0))["attachments"][0]["reason"],
        "not_regular_file"
    );
}

#[cfg(unix)]
#[test]
fn attachment_destination_directory_symlink_does_not_write_outside_archive() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    let image = fixture.write("media/image.png", b"image");
    fixture.write("codex/sessions/one.jsonl", codex("one", "A"));
    let first = fixture.collect(true);
    let path = archive(&first, 0);
    fs::create_dir_all(fixture.path("outside")).unwrap();
    symlink(
        fixture.path("outside"),
        path.parent().unwrap().join("attachments"),
    )
    .unwrap();
    fixture.write(
        "codex/sessions/one.jsonl",
        format!(
            "{}\n{}\n",
            json!({"type":"session_meta","payload":{"id":"one"}}),
            json!({"type":"input_image","image_url":image})
        ),
    );
    let report = fixture.collect(false);
    assert_eq!(report["errors"], 1);
    assert_eq!(raw(&path), codex("one", "A"));
    assert_eq!(fs::read_dir(fixture.path("outside")).unwrap().count(), 0);
}

#[test]
fn stale_sidecar_recovers_missing_original_attachment_after_gzip_first_crash() {
    let fixture = Fixture::new();
    let image = fixture.write("media/image.png", b"image");
    let transcript = format!(
        "{}\n{}\n",
        json!({"type":"session_meta","payload":{"id":"one"}}),
        json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_image","image_url":image}]}})
    );
    fixture.write("codex/sessions/one.jsonl", &transcript);
    let first = fixture.collect(true);
    let path = archive(&first, 0);
    let old_metadata = fs::read(sidecar(&path)).unwrap();
    let old_mapping = metadata(&path)["attachments"].clone();
    let latest = format!(
        "{transcript}{{\"type\":\"event_msg\",\"payload\":{{\"type\":\"agent_message\",\"message\":\"Added later\"}}}}\n"
    );
    fixture.write("codex/sessions/one.jsonl", &latest);
    fixture.collect(true);
    let gzip_before = fs::read(&path).unwrap();
    fs::write(sidecar(&path), old_metadata).unwrap();
    fs::remove_file(image).unwrap();
    let repaired = fixture.collect(true);
    assert_eq!(repaired["omissions"], 0);
    assert_eq!(repaired["updated"], 1);
    assert_eq!(metadata(&path)["attachments"], old_mapping);
    assert_eq!(metadata(&path)["raw_bytes"], latest.len());
    assert_eq!(raw(&path), latest.as_bytes());
    assert_eq!(fs::read(&path).unwrap(), gzip_before);
    assert_eq!(fixture.collect(true)["unchanged"], 1);
}

#[test]
fn stale_attachment_metadata_cannot_reuse_wrong_hash_size_path_or_source() {
    for invalid in ["hash", "size", "path", "source"] {
        let fixture = Fixture::new();
        let image = fixture.write("media/image.png", b"image");
        fixture.write(
            "codex/sessions/one.jsonl",
            format!(
                "{}\n{}\n",
                json!({"type":"session_meta","payload":{"id":"one"}}),
                json!({"type":"input_image","image_url":image})
            ),
        );
        let first = fixture.collect(true);
        let path = archive(&first, 0);
        let mut stale = metadata(&path);
        stale["raw_sha256"] = json!("0".repeat(64));
        match invalid {
            "hash" => stale["attachments"][0]["sha256"] = json!("0".repeat(64)),
            "size" => stale["attachments"][0]["bytes"] = json!(1),
            "path" => stale["attachments"][0]["path"] = json!("attachments/../../outside.png"),
            "source" => stale["source_id"] = json!("0".repeat(64)),
            _ => unreachable!(),
        }
        fs::write(sidecar(&path), serde_json::to_vec(&stale).unwrap()).unwrap();
        fs::remove_file(image).unwrap();
        let report = fixture.collect(true);
        assert_eq!(report["omissions"], 1, "invalid {invalid}");
        assert_eq!(
            metadata(&path)["attachments"][0]["reason"],
            "missing",
            "invalid {invalid}"
        );
    }
}

#[test]
fn only_native_message_text_and_media_blocks_discover_attachments() {
    let fixture = Fixture::new();
    let image = fixture.write("media/visible.png", b"visible");
    let hidden = fixture.write("media/private.txt", b"must not be copied");
    let link = format!("[visible]({})", image.display());
    let hidden_link = format!("[credentials]({})", hidden.display());
    let codex_records = [
        json!({"type":"session_meta","payload":{"id":"one"}}),
        json!({"type":"event_msg","payload":{"type":"item_completed","item":{"type":"UserMessage","content":[{"type":"text","text":link}]}}}),
        json!({"type":"event_msg","payload":{"type":"item_completed","item":{"type":"AgentMessage","content":[{"type":"text","text":link}]}}}),
        json!({"type":"event_msg","payload":{"type":"agent_message","message":link}}),
        json!({"type":"event_msg","payload":{"type":"user_message","message":link,"local_images":[image]}}),
        json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":link}]}}),
        json!({"type":"response_item","payload":{"type":"function_call_output","output":hidden_link}}),
        json!({"type":"response_item","payload":{"type":"function_call","arguments":{"text":hidden_link,"image_path":hidden,"content":[{"type":"input_image","image_url":hidden}]}}}),
        json!({"type":"event_msg","payload":{"type":"item_completed","item":{"type":"CommandExecution","output":hidden_link}}}),
    ];
    let claude_records = [
        json!({"type":"user","sessionId":"two","message":{"content":[{"type":"text","text":link},{"type":"image","source":{"type":"file","path":image}},{"type":"tool_result","content":[{"type":"text","text":hidden_link,"image_path":hidden}]}]}}),
        json!({"type":"assistant","sessionId":"two","message":{"content":[{"type":"text","text":link},{"type":"tool_use","input":{"text":hidden_link,"image_path":hidden}}]}}),
    ];
    let codex_bytes = codex_records
        .iter()
        .map(|r| format!("{r}\n"))
        .collect::<String>();
    let claude_bytes = claude_records
        .iter()
        .map(|r| format!("{r}\n"))
        .collect::<String>();
    fixture.write("codex/sessions/one.jsonl", &codex_bytes);
    fixture.write("claude/projects/project/two.jsonl", &claude_bytes);
    let report = fixture.collect(true);
    assert_eq!(report["updated"], 2);
    for index in 0..2 {
        let path = archive(&report, index);
        let meta = metadata(&path);
        assert_eq!(meta["attachments"].as_array().unwrap().len(), 1);
        assert_eq!(meta["attachments"][0]["reference"], image.to_str().unwrap());
        assert_eq!(meta["attachments"][0]["status"], "copied");
        assert_eq!(
            fs::read_dir(path.parent().unwrap().join("attachments"))
                .unwrap()
                .count(),
            1
        );
        let expected = if meta["provider"] == "codex" {
            &codex_bytes
        } else {
            &claude_bytes
        };
        assert_eq!(raw(&path), expected.as_bytes());
    }
}

#[test]
fn oversized_attachment_metadata_falls_back_without_losing_native_transcript() {
    let fixture = Fixture::new();
    fixture.write("codex/sessions/one.jsonl", codex("one", "before"));
    let first = fixture.collect(true);
    let path = archive(&first, 0);
    let reference = format!("https://example.test/{}.png", "a".repeat(2_000_000));
    let native = format!(
        "{}\n{}\n",
        json!({"type":"session_meta","payload":{"id":"one"}}),
        json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_image","image_url":reference}]}})
    );
    fixture.write("codex/sessions/one.jsonl", &native);
    let report = fixture.collect(true);
    assert_eq!(report["updated"], 1);
    assert_eq!(report["omissions"], 1);
    assert!(
        report["sources"][0]["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|diagnostic| diagnostic
                .as_str()
                .unwrap()
                .contains("1 attachment mappings omitted from sidecar"))
    );
    assert_eq!(metadata(&path)["attachments"], json!([]));
    assert_eq!(metadata(&path)["raw_bytes"], native.len());
    assert_eq!(raw(&path), native.as_bytes());
}

#[test]
fn each_native_message_lane_copies_its_markdown_attachment() {
    for lane in 0..8 {
        let fixture = Fixture::new();
        let file = fixture.write("media/report.txt", b"report");
        let text = format!("[report]({})", file.display());
        let record = match lane {
            0 => {
                json!({"type":"event_msg","payload":{"type":"item_completed","item":{"type":"UserMessage","content":[{"type":"text","text":text}]}}})
            }
            1 => {
                json!({"type":"event_msg","payload":{"type":"item_completed","item":{"type":"AgentMessage","content":[{"type":"text","text":text}]}}})
            }
            2 => json!({"type":"event_msg","payload":{"type":"user_message","message":text}}),
            3 => json!({"type":"event_msg","payload":{"type":"agent_message","text":text}}),
            4 => {
                json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":text}]}})
            }
            5 => {
                json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":text}]}})
            }
            6 => json!({"type":"user","sessionId":"one","message":{"content":text}}),
            7 => {
                json!({"type":"assistant","sessionId":"one","message":{"content":[{"type":"text","text":text}]}})
            }
            _ => unreachable!(),
        };
        let source = if lane < 6 {
            "codex/sessions/one.jsonl"
        } else {
            "claude/projects/project/one.jsonl"
        };
        fixture.write(source, format!("{record}\n"));
        let report = fixture.collect(true);
        let path = archive(&report, 0);
        let meta = metadata(&path);
        assert_eq!(
            meta["attachments"].as_array().unwrap().len(),
            1,
            "lane {lane}"
        );
        assert_eq!(meta["attachments"][0]["status"], "copied", "lane {lane}");
        assert_eq!(
            fs::read(
                path.parent()
                    .unwrap()
                    .join(meta["attachments"][0]["path"].as_str().unwrap())
            )
            .unwrap(),
            b"report",
            "lane {lane}"
        );
    }
}

#[test]
fn claude_tool_result_copies_typed_media_without_reading_text_or_arguments() {
    let fixture = Fixture::new();
    let screenshot = fixture.write("media/screenshot.png", b"screenshot");
    let secret = fixture.write("media/private.txt", b"private");
    let link = format!("[credentials]({})", secret.display());
    let record = json!({"type":"user","sessionId":"one","message":{"content":[
        {"type":"tool_result","content":[
            {"type":"text","text":link,"image_path":secret},
            {"type":"image","source":{"type":"file","path":screenshot}},
            {"type":"tool_result","content":[{"type":"image","source":{"type":"file","path":secret}}]}
        ]},
        {"type":"tool_result","content":link},
        {"type":"tool_use","input":{"type":"image","source":{"type":"file","path":secret}}}
    ]}});
    let native = format!("{record}\n");
    fixture.write("claude/projects/project/one.jsonl", &native);
    let report = fixture.collect(true);
    let path = archive(&report, 0);
    let meta = metadata(&path);
    assert_eq!(meta["attachments"].as_array().unwrap().len(), 1);
    assert_eq!(
        meta["attachments"][0]["reference"],
        screenshot.to_str().unwrap()
    );
    assert_eq!(meta["attachments"][0]["status"], "copied");
    assert_eq!(
        fs::read(
            path.parent()
                .unwrap()
                .join(meta["attachments"][0]["path"].as_str().unwrap())
        )
        .unwrap(),
        b"screenshot"
    );
    assert_eq!(
        fs::read_dir(path.parent().unwrap().join("attachments"))
            .unwrap()
            .count(),
        1
    );
    assert_eq!(raw(&path), native.as_bytes());
}

#[test]
fn codex_current_capitalized_text_and_media_blocks_copy_attachments() {
    let fixture = Fixture::new();
    let text_file = fixture.write("media/report.txt", b"report");
    let image = fixture.write("media/image.png", b"image");
    let local = fixture.write("media/local.png", b"local");
    let record = json!({"type":"event_msg","payload":{"type":"item_completed","item":{"type":"AgentMessage","content":[
        {"type":"Text","text":format!("[report]({})", text_file.display())},
        {"type":"Image","image_url":image},
        {"type":"LocalImage","path":local}
    ]}}});
    fixture.write("codex/sessions/one.jsonl", format!("{record}\n"));
    let report = fixture.collect(true);
    let path = archive(&report, 0);
    let meta = metadata(&path);
    let attachments = meta["attachments"].as_array().unwrap();
    assert_eq!(attachments.len(), 3);
    for (reference, expected) in [
        (text_file, b"report".as_slice()),
        (image, b"image".as_slice()),
        (local, b"local".as_slice()),
    ] {
        let attachment = attachments
            .iter()
            .find(|a| a["reference"] == reference.to_str().unwrap())
            .unwrap();
        assert_eq!(attachment["status"], "copied");
        assert_eq!(
            fs::read(
                path.parent()
                    .unwrap()
                    .join(attachment["path"].as_str().unwrap())
            )
            .unwrap(),
            expected
        );
    }
}

#[test]
fn copied_attachment_mapping_survives_rewrites_growth_and_narrowed_policy() {
    let fixture = Fixture::new();
    let image = fixture.write("media/screenshot.png", b"first image");
    fixture.write(
        "codex/sessions/one.jsonl",
        format!(
            "{}\n{}\n",
            json!({"type":"session_meta","payload":{"id":"one"}}),
            json!({"type":"input_image","image_url":image})
        ),
    );
    let first = fixture.collect(true);
    let path = archive(&first, 0);
    let original = metadata(&path);
    let copied_path = path
        .parent()
        .unwrap()
        .join(original["attachments"][0]["path"].as_str().unwrap());
    for replacement in [
        b"later image".as_slice(),
        b"a much larger later image".as_slice(),
    ] {
        fs::write(&image, replacement).unwrap();
        assert_eq!(fixture.collect(true)["unchanged"], 1);
        assert_eq!(metadata(&path), original);
    }
    fixture.config(1, 200_000_000);
    let config = fs::read_to_string(fixture.path("config.toml"))
        .unwrap()
        .replace(
            &format!("attachment_roots = [{:?}]", fixture.path("media")),
            "attachment_roots = []",
        );
    fs::write(fixture.path("config.toml"), config).unwrap();
    let last = fixture.collect(true);
    assert_eq!(last["unchanged"], 1);
    assert_eq!(last["omissions"], 0);
    assert_eq!(metadata(&path), original);
    assert_eq!(fs::read(copied_path).unwrap(), b"first image");
}

#[test]
fn changing_attachment_sources_do_not_abort_complete_transcript_capture() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let fixture = Fixture::new();
    let mut paths = Vec::new();
    for index in 0..64 {
        paths.push(fixture.write(&format!("media/image-{index}.png"), b"initial"));
    }
    let native = format!(
        "{}\n{}\n",
        json!({"type":"session_meta","payload":{"id":"one"}}),
        json!({"type":"response_item","payload":{"type":"message","role":"user","content":paths.iter().map(|path| json!({"type":"input_image","image_url":path})).collect::<Vec<_>>()}})
    );
    fixture.write("codex/sessions/one.jsonl", &native);
    let running = Arc::new(AtomicBool::new(true));
    let worker_running = running.clone();
    let worker = std::thread::spawn(move || {
        while worker_running.load(Ordering::Relaxed) {
            for path in &paths {
                let _ = fs::remove_file(path);
                let _ = fs::write(path, b"replacement");
            }
        }
    });
    let output = fixture.run("collect");
    running.store(false, Ordering::Relaxed);
    worker.join().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["updated"], 1);
    assert_eq!(report["errors"], 0);
    assert_eq!(raw(&archive(&report, 0)), native.as_bytes());
}

#[test]
fn markdown_attachment_parser_handles_parentheses_unicode_spaces_and_percent_encoding() {
    let fixture = Fixture::new();
    let parenthesized = fixture.write("media/a(b).png", b"parentheses");
    let spaced = fixture.write("media/space image.png", b"space");
    let unicode = fixture.write("media/日本語.png", b"unicode");
    let percent = fixture.write("media/literal%41.png", b"percent");
    let ignored = fixture.write("media/code-only.png", b"code");
    let encoded_space = spaced.to_str().unwrap().replace(' ', "%20");
    let encoded_percent = percent.to_str().unwrap().replace('%', "%25");
    let text = format!(
        "![angle](<{}>)\n![balanced]({})\n![space](<{}>)\n[encoded report]({})\n![unicode](<{}>)\n![percent]({})\n\n```markdown\n![ignored]({})\n```\n`![ignored]({})`\n",
        parenthesized.display(),
        parenthesized.display(),
        spaced.display(),
        encoded_space,
        unicode.display(),
        encoded_percent,
        ignored.display(),
        ignored.display()
    );
    let native = format!(
        "{}\n",
        json!({"type":"event_msg","payload":{"type":"agent_message","message":text}})
    );
    fixture.write("codex/sessions/one.jsonl", &native);
    let report = fixture.collect(true);
    assert_eq!(report["omissions"], 0);
    let path = archive(&report, 0);
    let meta = metadata(&path);
    let mappings = meta["attachments"].as_array().unwrap();
    assert_eq!(mappings.len(), 5);
    for (reference, bytes) in [
        (parenthesized.to_str().unwrap(), b"parentheses".as_slice()),
        (spaced.to_str().unwrap(), b"space".as_slice()),
        (encoded_space.as_str(), b"space".as_slice()),
        (unicode.to_str().unwrap(), b"unicode".as_slice()),
        (encoded_percent.as_str(), b"percent".as_slice()),
    ] {
        let mapping = mappings
            .iter()
            .find(|mapping| mapping["reference"] == reference)
            .unwrap_or_else(|| panic!("missing reference {reference}"));
        assert_eq!(mapping["status"], "copied");
        assert_eq!(
            fs::read(
                path.parent()
                    .unwrap()
                    .join(mapping["path"].as_str().unwrap())
            )
            .unwrap(),
            bytes
        );
    }
    assert_eq!(raw(&path), native.as_bytes());
}

#[test]
fn native_media_path_preserves_literal_percent_filename() {
    let fixture = Fixture::new();
    let image = fixture.write("media/image%41.png", b"literal percent");
    fixture.write("media/imageA.png", b"wrong decoded file");
    fixture.write(
        "codex/sessions/one.jsonl",
        format!("{}\n", json!({"type":"input_image","image_url":image})),
    );
    let report = fixture.collect(true);
    let path = archive(&report, 0);
    let meta = metadata(&path);
    assert_eq!(meta["attachments"][0]["reference"], image.to_str().unwrap());
    assert_eq!(meta["attachments"][0]["status"], "copied");
    assert_eq!(
        fs::read(
            path.parent()
                .unwrap()
                .join(meta["attachments"][0]["path"].as_str().unwrap())
        )
        .unwrap(),
        b"literal percent"
    );
}
