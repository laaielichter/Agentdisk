use std::fs;
use std::path::PathBuf;
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
