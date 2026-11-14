use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn tmp(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let d = std::env::temp_dir().join(format!(
        "agentdisk-cli-{tag}-{}-{nanos}",
        std::process::id()
    ));
    fs::create_dir_all(&d).unwrap();
    d
}

/// The binary with an isolated home, so tests never touch the real one.
fn bin(home: &PathBuf) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_agentdisk"));
    c.env("HOME", home)
        .env("USERPROFILE", home)
        .env("APPDATA", home.join("AppData/Roaming"))
        .env("LOCALAPPDATA", home.join("AppData/Local"))
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .env("AGENTDISK_HOME", home.join("agentdisk"))
        .env_remove("CLAUDE_CONFIG_DIR")
        .env_remove("CODEX_HOME")
        .stdin(Stdio::null());
    c
}

fn run(home: &PathBuf, args: &[&str]) -> (i32, String, String) {
    let out = bin(home).args(args).output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn version_and_help() {
    let h = tmp("v");
    let (code, out, _) = run(&h, &["--version"]);
    assert_eq!(code, 0);
    assert!(out.contains(env!("CARGO_PKG_VERSION")));
    let (code, out, _) = run(&h, &["--help"]);
    assert_eq!(code, 0);
    assert!(out.contains("USAGE"));
    let (code, _, err) = run(&h, &["--delete-everything"]);
    assert_eq!(code, 2);
    assert!(err.contains("unknown command"));
    fs::remove_dir_all(&h).unwrap();
}

#[test]
fn config_reads_bom_prefixed_file() {
    let h = tmp("config-bom");
    let config_dir = h.join("agentdisk");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("config.toml"),
        "\u{feff}worktree_days = 3\n",
    )
    .unwrap();

    let (code, out, err) = run(&h, &["config"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(
        out.contains("worktree_days = 3, build_days = 14, max_depth = 6"),
        "{out}"
    );
    assert!(!err.contains("unknown key"), "{err}");
    fs::remove_dir_all(&h).unwrap();
}

#[test]
fn scan_finds_fake_agent_data_and_is_read_only() {
    let h = tmp("scan");
    fs::create_dir_all(h.join(".claude/projects/x")).unwrap();
    fs::write(h.join(".claude/projects/x/s.jsonl"), vec![b'x'; 4096]).unwrap();
    fs::create_dir_all(h.join(".cursor/projects")).unwrap();
    fs::write(h.join(".cursor/projects/t.json"), b"{}").unwrap();
    let (code, out, err) = run(&h, &["scan", "--json"]);
    assert_eq!(code, 0, "{err}");
    let v = agentdisk_core::json::parse(&out).unwrap();
    let agents = v.get("agents").unwrap().as_arr().unwrap();
    let ids: Vec<&str> = agents
        .iter()
        .map(|a| a.get("adapter").unwrap().as_str().unwrap())
        .collect();
    assert!(ids.contains(&"claude-code"), "{ids:?}");
    assert!(ids.contains(&"cursor"), "{ids:?}");
    assert_eq!(v.get("schema_version").unwrap().as_u64(), Some(1));
    // Nothing written by a scan.
    assert!(!h.join("agentdisk").exists());
    let (code, text, _) = run(&h, &["scan"]);
    assert_eq!(code, 0);
    assert!(text.contains("Claude Code"));
    fs::remove_dir_all(&h).unwrap();
}

#[test]
fn size_command() {
    let h = tmp("size");
    fs::write(h.join("f"), vec![0u8; 3000]).unwrap();
    let (code, out, _) = run(&h, &["size", h.to_str().unwrap(), "--json"]);
    assert_eq!(code, 0);
    let v = agentdisk_core::json::parse(out.trim()).unwrap();
    assert_eq!(v.get("logical_bytes").unwrap().as_u64(), Some(3000));
    fs::remove_dir_all(&h).unwrap();
}

#[test]
fn apply_and_purge_refuse_without_a_terminal() {
    let h = tmp("apply");
    let (code, _, _) = run(&h, &["plan"]);
    assert_eq!(code, 0);
    // Make a fake plan with one item so apply gets to the confirmation.
    let dir = h.join("agentdisk/plans");
    let victim = h.join("victim");
    fs::create_dir_all(&victim).unwrap();
    fs::write(
        dir.join("20990101-abcdef.json"),
        format!(
            r#"{{"id":"20990101-abcdef","created":0,"items":[{{"kind":"agent-cache","path":{:?},"bytes":1,"newest_mtime":99999999999}}]}}"#,
            victim.to_str().unwrap()
        ),
    )
    .unwrap();
    let (code, _, err) = run(&h, &["apply", "20990101-abcdef"]);
    assert_eq!(code, 2);
    assert!(err.contains("interactive terminal"), "{err}");
    assert!(victim.exists(), "nothing may move without a human");
    let (code, _, _) = run(&h, &["purge", "--older-than", "0"]);
    assert_eq!(code, 0); // nothing in quarantine
    fs::remove_dir_all(&h).unwrap();
}

#[test]
fn plans_lists_saved_plans_newest_first() {
    let h = tmp("plans");
    let (code, out, err) = run(&h, &["plans"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("No saved plans"), "{out}");
    assert!(!h.join("agentdisk").exists(), "plans must not write");

    for _ in 0..2 {
        let (code, _, err) = run(&h, &["plan"]);
        assert_eq!(code, 0, "{err}");
    }
    let dir = h.join("agentdisk/plans");
    let made: Vec<String> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .map(|n| n.trim_end_matches(".json").to_string())
        .collect();
    assert_eq!(made.len(), 2, "{made:?}");

    // A newer plan that was applied, and a file that is not a plan.
    fs::write(
        dir.join("20990101-abcdef.json"),
        r#"{"id":"20990101-abcdef","created":4070908800,"items":[{"kind":"build-folder","path":"x","bytes":2048}]}"#,
    )
    .unwrap();
    let q = h.join("agentdisk/quarantine/20990101-abcdef");
    write_journal(&q, &[(&h.join("x"), &q.join("000-x"))]);
    fs::write(dir.join("broken.json"), "{ nope").unwrap();
    let before = fs::read_dir(&dir).unwrap().count();

    let (code, out, err) = run(&h, &["plans"]);
    assert_eq!(code, 0, "{err}");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 5, "{out}");
    assert!(lines[1].contains("20990101-abcdef"), "{out}");
    assert!(
        lines[1].contains("applied (restore with: agentdisk restore 20990101-abcdef)"),
        "{out}"
    );
    for id in &made {
        let line = lines.iter().find(|l| l.contains(id.as_str())).unwrap();
        assert!(line.contains("empty"), "{out}");
    }
    assert!(
        lines[4].contains("broken") && lines[4].contains("unreadable"),
        "{out}"
    );

    let (code, out, err) = run(&h, &["plans", "--json"]);
    assert_eq!(code, 0, "{err}");
    let v = agentdisk_core::json::parse(&out).unwrap();
    assert_eq!(v.get("schema_version").unwrap().as_u64(), Some(1));
    let plans = v.get("plans").unwrap().as_arr().unwrap();
    assert_eq!(plans.len(), 4);
    assert_eq!(plans[0].get("state").unwrap().as_str(), Some("applied"));
    assert_eq!(plans[0].get("total_bytes").unwrap().as_u64(), Some(2048));
    assert_eq!(plans[3].get("state").unwrap().as_str(), Some("unreadable"));

    assert_eq!(fs::read_dir(&dir).unwrap().count(), before);
    fs::remove_dir_all(&h).unwrap();
}

/// Write a quarantine journal by hand: (original, quarantine) pairs.
fn write_journal(dir: &Path, entries: &[(&Path, &Path)]) {
    use agentdisk_core::json::Value;
    let v: Vec<Value> = entries
        .iter()
        .map(|(original, quarantine)| {
            Value::obj(vec![
                ("original", original.to_str().unwrap().into()),
                ("quarantine", quarantine.to_str().unwrap().into()),
                ("kind", "build-folder".into()),
                ("bytes", 10u64.into()),
                ("moved_at", 0u64.into()),
                ("state", "quarantined".into()),
            ])
        })
        .collect();
    fs::create_dir_all(dir).unwrap();
    fs::write(dir.join("journal.json"), Value::Arr(v).pretty()).unwrap();
}

#[test]
fn restore_and_purge_refuse_bad_plan_ids() {
    let h = tmp("badid");
    // A journal outside the quarantine folder, reachable as `../x`.
    let stray = h.join("agentdisk/x/000-item");
    fs::create_dir_all(&stray).unwrap();
    let back = h.join("back");
    write_journal(&h.join("agentdisk/x"), &[(&back, &stray)]);
    for id in ["../x", "..\\x"] {
        let (code, out, err) = run(&h, &["restore", id]);
        assert_eq!(code, 2, "{out}{err}");
        assert!(err.contains("bad plan id"), "{err}");
        let (code, out, err) = run(&h, &["purge", "--plan", id, "--older-than", "0"]);
        assert_eq!(code, 2, "{out}{err}");
        assert!(err.contains("bad plan id"), "{err}");
        assert!(!out.contains("deleted for good"), "{out}");
    }
    assert!(stray.exists() && !back.exists(), "nothing may move");
    fs::remove_dir_all(&h).unwrap();
}

#[test]
fn purge_lists_quarantine_paths_and_leaves_outside_paths_alone() {
    let h = tmp("purgelist");
    let plan = h.join("agentdisk/quarantine/20990101-abcdef");
    let inside = plan.join("000-node_modules");
    fs::create_dir_all(&inside).unwrap();
    let precious = h.join("precious");
    fs::create_dir_all(&precious).unwrap();
    fs::write(precious.join("keep.txt"), b"keep").unwrap();
    write_journal(
        &plan,
        &[
            (&h.join("app/node_modules"), &inside),
            (&h.join("app/other"), &precious),
        ],
    );
    let inside_s = inside.to_str().unwrap();
    let precious_s = precious.to_str().unwrap();

    // The confirmation lists what will really be deleted.
    let (code, out, err) = run(&h, &["purge", "--older-than", "0"]);
    assert_eq!(code, 2, "{out}{err}");
    assert!(err.contains("interactive terminal"), "{err}");
    assert!(out.contains("1 item(s)"), "{out}");
    assert!(out.contains(inside_s), "{out}");
    assert!(!out.contains(precious_s), "{out}");
    assert!(
        err.contains(&format!("{precious_s}: not inside the quarantine")),
        "{err}"
    );

    // Same in a dry run, which also reports the refused entry in its exit code.
    let (code, out, err) = run(&h, &["purge", "--older-than", "0", "--dry-run"]);
    assert_eq!(code, 1, "{out}{err}");
    assert!(out.contains(inside_s) && !out.contains(precious_s), "{out}");

    // Restore leaves the outside entry alone too.
    let (code, out, err) = run(&h, &["restore", "20990101-abcdef"]);
    assert_eq!(code, 1, "{out}{err}");
    assert!(
        err.contains(&format!("{precious_s}: not inside the quarantine")),
        "{err}"
    );
    assert!(h.join("app/node_modules").is_dir(), "{out}{err}");
    assert!(precious.join("keep.txt").exists() && !h.join("app/other").exists());
    fs::remove_dir_all(&h).unwrap();
}

#[test]
fn mcp_over_stdio() {
    use std::io::Write;
    let h = tmp("mcp");
    let mut child = bin(&h)
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    {
        let stdin = child.stdin.as_mut().unwrap();
        writeln!(stdin, r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":"2025-06-18","capabilities":{{}},"clientInfo":{{"name":"t","version":"1"}}}}}}"#).unwrap();
        writeln!(
            stdin,
            r#"{{"jsonrpc":"2.0","method":"notifications/initialized"}}"#
        )
        .unwrap();
        writeln!(stdin, r#"{{"jsonrpc":"2.0","id":2,"method":"tools/list"}}"#).unwrap();
        writeln!(stdin, r#"{{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{{"name":"agentdisk_check","arguments":{{}}}}}}"#).unwrap();
    }
    drop(child.stdin.take());
    let out = child.wait_with_output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 3, "{text}");
    for l in &lines {
        agentdisk_core::json::parse(l).unwrap();
    }
    assert!(lines[1].contains("agentdisk_scan"));
    assert!(lines[2].contains("agentdisk"));
    fs::remove_dir_all(&h).unwrap();
}

#[test]
fn adapters_and_check() {
    let h = tmp("adapters");
    let (code, out, _) = run(&h, &["adapters"]);
    assert_eq!(code, 0);
    assert!(out.contains("Claude Code") && out.contains("Cursor"));
    let (code, out, _) = run(&h, &["check", "--json"]);
    assert_eq!(code, 0);
    agentdisk_core::json::parse(out.trim()).unwrap();
    fs::remove_dir_all(&h).unwrap();
}

#[test]
fn explain_command() {
    let h = tmp("explain");
    let web = h.join("web");
    fs::create_dir_all(web.join("node_modules/a")).unwrap();
    fs::write(web.join("package.json"), "{}").unwrap();
    fs::create_dir_all(h.join(".claude/projects/p")).unwrap();
    let (code, out, err) = run(
        &h,
        &[
            "explain",
            web.join("node_modules").to_str().unwrap(),
            "--json",
        ],
    );
    assert_eq!(code, 0, "{err}");
    let v = agentdisk_core::json::parse(out.trim()).unwrap();
    assert_eq!(v.get("verdict").unwrap().as_str(), Some("keep")); // changed today
    let (code, out, _) = run(
        &h,
        &["explain", h.join(".claude/projects").to_str().unwrap()],
    );
    assert_eq!(code, 0);
    assert!(out.contains("Claude Code") && out.contains("keep"), "{out}");
    let (code, _, _) = run(&h, &["explain", h.join("nope").to_str().unwrap()]);
    assert_eq!(code, 2);
    fs::remove_dir_all(&h).unwrap();
}

#[test]
fn size_and_explain_with_several_folders() {
    let h = tmp("several");
    let (a, b, nope) = (h.join("a"), h.join("b"), h.join("nope"));
    for d in [&a, &b] {
        fs::create_dir_all(d).unwrap();
        fs::write(d.join("f"), "hi").unwrap();
    }
    let s = |p: &PathBuf| p.to_str().unwrap().to_string();
    let json = |v: &str| agentdisk_core::json::parse(v.trim()).unwrap();
    for cmd in ["size", "explain"] {
        let call = |dirs: &[&PathBuf], flags: &[&str]| {
            let mut args = vec![cmd.to_string()];
            args.extend(dirs.iter().map(|d| s(d)));
            args.extend(flags.iter().map(|f| f.to_string()));
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            run(&h, &args)
        };

        // One folder: a single object.
        let (code, out, err) = call(&[&a], &["--json"]);
        assert_eq!(code, 0, "{cmd}: {err}");
        assert!(json(&out).get("path").is_some(), "{cmd}: {out}");

        // Several: one array, in the order given.
        let (code, out, err) = call(&[&b, &a], &["--json"]);
        assert_eq!(code, 0, "{cmd}: {err}");
        let v = json(&out);
        let paths: Vec<&str> = v
            .as_arr()
            .unwrap()
            .iter()
            .map(|e| e.get("path").unwrap().as_str().unwrap())
            .collect();
        assert_eq!(paths.len(), 2, "{cmd}: {out}");
        assert!(
            paths[0].ends_with('b') && paths[1].ends_with('a'),
            "{paths:?}"
        );

        // A missing folder in the middle is reported, the others still run.
        let (code, out, _) = call(&[&a, &nope, &b], &["--json"]);
        assert_eq!(code, 1, "{cmd}: {out}");
        let v = json(&out);
        let list = v.as_arr().unwrap();
        assert_eq!(list.len(), 3, "{cmd}: {out}");
        assert!(list[0].get("error").is_none() && list[2].get("error").is_none());
        assert!(list[1].get("error").unwrap().as_str().is_some());
        assert!(list[1]
            .get("path")
            .unwrap()
            .as_str()
            .unwrap()
            .ends_with("nope"));

        // Text: the error goes to stderr, the other folders still print.
        let (code, out, err) = call(&[&a, &nope, &b], &[]);
        assert_eq!(code, 1, "{cmd}: {out}{err}");
        assert!(err.contains("nope"), "{cmd}: {err}");
        // `explain` prints the canonical path, which can differ from the one
        // given (short Windows temp names), so match on the folder name.
        let printed = |n: &str| {
            out.lines()
                .any(|l| l.ends_with(&format!("/{n}")) || l.ends_with(&format!("\\{n}")))
        };
        assert!(printed("a") && printed("b"), "{cmd}: {out}");

        // One missing folder keeps today's behaviour.
        let (code, out, err) = call(&[&nope], &["--json"]);
        assert_eq!(code, 2, "{cmd}: {out}{err}");
        assert!(out.is_empty() && err.contains("nope"), "{cmd}: {out}{err}");
    }
    fs::remove_dir_all(&h).unwrap();
}

#[test]
fn manifest_versions_match_cargo() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for f in [
        ".claude-plugin/plugin.json",
        ".claude-plugin/marketplace.json",
        "gemini-extension.json",
    ] {
        let text = fs::read_to_string(root.join(f)).unwrap();
        let needle = format!("\"version\": \"{}\"", env!("CARGO_PKG_VERSION"));
        assert!(text.contains(&needle), "{f} must contain {needle}");
        agentdisk_core::json::parse(&text).unwrap();
    }
    let changelog = fs::read_to_string(root.join("CHANGELOG.md")).unwrap();
    assert!(changelog.contains(&format!("## {}", env!("CARGO_PKG_VERSION"))));
}
