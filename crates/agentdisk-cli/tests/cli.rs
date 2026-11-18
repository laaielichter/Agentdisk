use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

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

fn files_under(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if entry.file_type().unwrap().is_dir() {
            files.extend(files_under(&path));
        } else {
            files.push(path);
        }
    }
    files.sort();
    files
}

fn set_mtime(path: &Path, modified: SystemTime) {
    let times = fs::FileTimes::new().set_modified(modified);
    #[cfg(windows)]
    let file = {
        use std::os::windows::fs::OpenOptionsExt;
        // Open directories with write-attributes access so their timestamp can be set too.
        fs::OpenOptions::new()
            .access_mode(0x100) // FILE_WRITE_ATTRIBUTES
            .custom_flags(0x0200_0000) // FILE_FLAG_BACKUP_SEMANTICS
            .open(path)
            .unwrap()
    };
    #[cfg(not(windows))]
    let file = fs::File::open(path).unwrap();
    file.set_times(times).unwrap();
}

fn read_one_line_then_close(mut command: Command) {
    use std::io::{BufRead, BufReader};
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let mut first_line = String::new();
    assert!(reader.read_line(&mut first_line).unwrap() > 0);
    drop(reader);
    let output = child.wait_with_output().unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn closed_stdout_stops_adapters_quietly() {
    let h = tmp("broken-adapters");
    let adapters = h.join("adapters");
    fs::create_dir_all(&adapters).unwrap();
    // More output than a pipe can hold ensures the writer is still active
    // when the reader closes after its first line.
    fs::write(
        adapters.join("large.toml"),
        format!(
            "id = \"large\"\nname = \"{}\"\nvendor = \"fixture\"\n",
            "x".repeat(1024 * 1024)
        ),
    )
    .unwrap();
    for json in [false, true] {
        let mut command = bin(&h);
        command.arg("adapters").arg("--adapters").arg(&adapters);
        if json {
            command.arg("--json");
        }
        read_one_line_then_close(command);
    }
    fs::remove_dir_all(&h).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn stdout_write_errors_other_than_a_closed_pipe_are_reported() {
    let h = tmp("full-output");
    let output = bin(&h)
        .arg("--help")
        .stdout(
            fs::OpenOptions::new()
                .write(true)
                .open("/dev/full")
                .unwrap(),
        )
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.starts_with("error: "), "{stderr}");
    assert!(!stderr.contains("panicked"), "{stderr}");
    fs::remove_dir_all(&h).unwrap();
}

#[test]
fn closed_stdout_stops_repeated_folder_output_quietly() {
    let h = tmp("broken-folders");
    let folder = h.join("folder");
    fs::create_dir_all(&folder).unwrap();
    for name in ["size", "explain"] {
        let mut command = bin(&h);
        command.current_dir(&h).arg(name);
        command.args(std::iter::repeat_n("folder", 1024));
        read_one_line_then_close(command);
    }
    fs::remove_dir_all(&h).unwrap();
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
fn subcommand_help_explains_each_command() {
    let h = tmp("subcommand-help");
    // Quarantine's help is covered by a separate issue.
    for command in [
        "scan", "size", "explain", "advise", "check", "adapters", "plans", "plan", "apply",
        "restore", "purge", "compact", "mcp", "config",
    ] {
        let (code, out, err) = run(&h, &[command, "--help"]);
        assert_eq!(code, 0, "{command}: {out}{err}");
        assert!(
            out.starts_with(&format!("agentdisk {command}")),
            "{command}: {out}"
        );
        if matches!(command, "advise" | "plan") {
            for flag in ["--depth", "--threads", "--adapters", "--no-agents"] {
                assert!(
                    out.contains(flag),
                    "{command} help is missing {flag}: {out}"
                );
            }
        }
    }
    fs::remove_dir_all(&h).unwrap();
}

#[test]
fn mcp_rejects_unknown_options() {
    let h = tmp("mcp-unknown-option");
    let (code, out, err) = run(&h, &["mcp", "--bogus"]);
    assert_eq!(code, 2, "{out}{err}");
    assert!(err.contains("unknown option '--bogus'"), "{err}");
    fs::remove_dir_all(&h).unwrap();
}

#[test]
fn advise_runs_through_the_cli_in_text_and_json() {
    let h = tmp("advise-cli");
    let (code, out, err) = run(&h, &["advise", "--no-agents"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(
        out.starts_with("agentdisk advise — ways to need less space"),
        "{out}"
    );
    assert!(err.is_empty(), "{err}");

    let (code, out, err) = run(&h, &["advise", "--no-agents", "--json"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(agentdisk_core::json::parse(&out).is_ok(), "{out}");
    assert!(err.is_empty(), "{err}");
    assert!(
        !h.join("agentdisk").exists(),
        "advise must not write config"
    );
    fs::remove_dir_all(&h).unwrap();
}

#[test]
fn compact_cli_refusals_and_dry_run_preserve_files() {
    let h = tmp("compact-cli");
    let (code, out, err) = run(&h, &["compact"]);
    assert_eq!(code, 2, "{out}{err}");
    assert!(out.starts_with("agentdisk compact"), "{out}");

    let workspace = h.join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    fs::write(workspace.join("keep.txt"), b"keep").unwrap();
    let before = files_under(&workspace);
    let (code, out, err) = run(
        &h,
        &["compact", workspace.to_str().unwrap(), "--algo", "bogus"],
    );
    assert_eq!(code, 2, "{out}{err}");
    assert!(err.contains("unknown --algo 'bogus'"), "{err}");
    assert_eq!(files_under(&workspace), before);

    let file = h.join("some-file.txt");
    fs::write(&file, b"unchanged").unwrap();
    let contents = fs::read(&file).unwrap();
    let (code, out, err) = run(&h, &["compact", file.to_str().unwrap()]);
    assert_eq!(code, 2, "{out}{err}");
    assert!(err.contains("give a folder"), "{err}");
    assert_eq!(fs::read(&file).unwrap(), contents);

    let fresh = h.join("fresh");
    fs::create_dir_all(&fresh).unwrap();
    fs::write(fresh.join("keep.txt"), b"fresh").unwrap();
    let before = files_under(&fresh);
    let (code, out, err) = run(&h, &["compact", fresh.to_str().unwrap(), "--dry-run"]);
    assert_eq!(code, 2, "{out}{err}");
    assert!(err.contains("changed") && err.contains("7+ days"), "{err}");
    assert_eq!(files_under(&fresh), before);

    let old = h.join("old");
    fs::create_dir_all(&old).unwrap();
    let old_file = old.join("keep.txt");
    fs::write(&old_file, b"old").unwrap();
    let previous = SystemTime::now()
        - Duration::from_secs((agentdisk_core::compact::MIN_IDLE_DAYS + 2) * 86_400);
    set_mtime(&old_file, previous);
    set_mtime(&old, previous);
    let before = files_under(&old);
    let contents = fs::read(&old_file).unwrap();
    let (code, out, err) = run(&h, &["compact", old.to_str().unwrap(), "--dry-run"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("Command:"), "{out}");
    assert_eq!(files_under(&old), before);
    assert_eq!(fs::read(&old_file).unwrap(), contents);
    fs::remove_dir_all(&h).unwrap();
}

#[test]
fn config_init_writes_once_and_then_reads_cleanly() {
    let h = tmp("config-init-cli");
    let path = h.join("agentdisk/config.toml");
    let (code, out, err) = run(&h, &["config", "--init"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(path.is_file(), "{out}{err}");
    let original = fs::read(&path).unwrap();

    let (code, out, err) = run(&h, &["config"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(err.is_empty(), "{err}");
    assert!(out.contains("worktree_days ="), "{out}");

    let (code, out, err) = run(&h, &["config", "--init"]);
    assert_eq!(code, 2, "{out}{err}");
    assert!(err.contains("already exists"), "{err}");
    assert_eq!(fs::read(&path).unwrap(), original);
    fs::remove_dir_all(&h).unwrap();
}

#[test]
fn quarantine_cli_lists_empty_and_populated_quarantine() {
    let h = tmp("quarantine-cli");
    let (code, out, err) = run(&h, &["quarantine"]);
    assert_eq!(code, 0, "{out}{err}");
    assert_eq!(out, "Quarantine is empty.\n");

    let id = "20990101-abcdef";
    let plan = h.join("agentdisk/quarantine").join(id);
    let original = h.join("app/node_modules");
    let quarantined = plan.join("000-node_modules");
    write_journal(&plan, &[(&original, &quarantined)]);
    let (code, out, err) = run(&h, &["quarantine"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("QUARANTINE") && out.contains(id), "{out}");
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
fn scan_recognizes_only_dart_tool_folders_with_pubspec() {
    let h = tmp("dart-tool");
    let projects = h.join("projects");
    let dart = projects.join("dart");
    let generated = dart.join(".dart_tool");
    fs::create_dir_all(&generated).unwrap();
    fs::write(generated.join("package_config.json"), "{}").unwrap();
    fs::write(dart.join("pubspec.yaml"), "name: fixture\n").unwrap();
    let unmarked = projects.join("notes/.dart_tool");
    fs::create_dir_all(&unmarked).unwrap();
    let (code, out, err) = run(
        &h,
        &[
            "scan",
            "-p",
            projects.to_str().unwrap(),
            "--no-agents",
            "--json",
        ],
    );
    assert_eq!(code, 0, "{out}{err}");
    let report = agentdisk_core::json::parse(&out).unwrap();
    let builds = report.get("build_folders").unwrap().as_arr().unwrap();
    assert_eq!(builds.len(), 1, "{out}");
    assert_eq!(builds[0].get("ecosystem").unwrap().as_str(), Some("dart"));
    assert_eq!(
        builds[0].get("path").unwrap().as_str(),
        Some(agentdisk_core::paths::display(&agentdisk_core::paths::canon(&generated)).as_str())
    );
    let (code, out, err) = run(&h, &["explain", generated.to_str().unwrap(), "--json"]);
    assert_eq!(code, 0, "{out}{err}");
    let explained = agentdisk_core::json::parse(&out).unwrap();
    assert_eq!(explained.get("owner").unwrap().as_str(), Some("dart"));
    assert!(generated.join("package_config.json").is_file());
    assert!(unmarked.is_dir());
    assert!(
        !h.join("agentdisk").exists(),
        "scan and explain must not write"
    );
    fs::remove_dir_all(&h).unwrap();
}

#[test]
fn scan_warns_when_a_project_path_is_a_file() {
    let h = tmp("project-file");
    let project = h.join("app");
    fs::create_dir_all(project.join("node_modules/dependency")).unwrap();
    let file = project.join("package.json");
    fs::write(&file, "{}").unwrap();
    let file_arg = file.to_str().unwrap();
    let warning = format!(
        "project path is not a folder: {}",
        agentdisk_core::paths::display(&file)
    );

    let (code, out, err) = run(&h, &["scan", "-p", file_arg, "--no-agents", "--json"]);
    assert_eq!(code, 0, "{out}{err}");
    let report = agentdisk_core::json::parse(&out).unwrap();
    assert_eq!(
        report.get("warnings").unwrap().as_arr().unwrap(),
        &[agentdisk_core::json::Value::from(warning.as_str())]
    );
    assert!(report
        .get("build_folders")
        .unwrap()
        .as_arr()
        .unwrap()
        .is_empty());

    let (code, out, err) = run(&h, &["scan", "-p", file_arg, "--no-agents"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains(&format!("WARNINGS\n  - {warning}")), "{out}");
    assert_eq!(fs::read_to_string(&file).unwrap(), "{}");
    assert!(!h.join("agentdisk").exists(), "a scan must not write");
    fs::remove_dir_all(&h).unwrap();
}

#[test]
fn scan_keeps_valid_roots_and_the_missing_folder_warning() {
    let h = tmp("project-roots");
    let project = h.join("app");
    fs::create_dir_all(project.join("node_modules/dependency")).unwrap();
    let file = project.join("package.json");
    fs::write(&file, "{}").unwrap();
    let missing = h.join("missing");
    let (code, out, err) = run(
        &h,
        &[
            "scan",
            "-p",
            file.to_str().unwrap(),
            "-p",
            project.to_str().unwrap(),
            "-p",
            missing.to_str().unwrap(),
            "--no-agents",
            "--json",
        ],
    );
    assert_eq!(code, 0, "{out}{err}");
    let report = agentdisk_core::json::parse(&out).unwrap();
    let warnings: Vec<_> = report
        .get("warnings")
        .unwrap()
        .as_arr()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(
        warnings,
        vec![
            format!(
                "project path is not a folder: {}",
                agentdisk_core::paths::display(&file)
            ),
            format!(
                "project folder not found: {}",
                agentdisk_core::paths::display(&missing)
            ),
        ]
    );
    assert_eq!(
        report.get("build_folders").unwrap().as_arr().unwrap().len(),
        1
    );
    assert!(!h.join("agentdisk").exists(), "a scan must not write");
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
    write_journal_at(dir, entries, 0);
}

fn write_journal_at(dir: &Path, entries: &[(&Path, &Path)], moved_at: u64) {
    write_journal_state(dir, entries, moved_at, "quarantined");
}

fn write_journal_state(dir: &Path, entries: &[(&Path, &Path)], moved_at: u64, state: &str) {
    use agentdisk_core::json::Value;
    let v: Vec<Value> = entries
        .iter()
        .map(|(original, quarantine)| {
            Value::obj(vec![
                ("original", original.to_str().unwrap().into()),
                ("quarantine", quarantine.to_str().unwrap().into()),
                ("kind", "build-folder".into()),
                ("bytes", 10u64.into()),
                ("moved_at", moved_at.into()),
                ("state", state.into()),
            ])
        })
        .collect();
    fs::create_dir_all(dir).unwrap();
    fs::write(dir.join("journal.json"), Value::Arr(v).pretty()).unwrap();
}

#[test]
fn purge_named_plan_reports_nothing_in_quarantine() {
    let h = tmp("purge-empty-plan");
    let id = "20990101-abcdef";
    let plan = h.join("agentdisk/quarantine").join(id);
    let inside = plan.join("000-node_modules");
    for state in [None, Some("restored"), Some("purged")] {
        if let Some(state) = state {
            write_journal_state(&plan, &[(&h.join("app/node_modules"), &inside)], 0, state);
        }
        let (code, out, err) = run(&h, &["purge", "--plan", id, "--dry-run"]);
        assert_eq!(code, 1, "{out}{err}");
        assert_eq!(
            out,
            format!(
                "Nothing in quarantine for plan '{id}'. See the plans with: agentdisk quarantine\n"
            )
        );
        assert_eq!(err, "");
    }
    write_journal(&plan, &[]);
    let (code, out, err) = run(&h, &["purge", "--plan", id, "--dry-run"]);
    assert_eq!(code, 1, "{out}{err}");
    assert_eq!(
        out,
        format!(
            "Nothing in quarantine for plan '{id}'. See the plans with: agentdisk quarantine\n"
        )
    );
    assert_eq!(err, "");
    let (code, out, err) = run(&h, &["purge", "--dry-run"]);
    assert_eq!(code, 0, "{out}{err}");
    assert_eq!(out, "Nothing in quarantine is older than 7 day(s).\n");
    assert_eq!(err, "");
    fs::remove_dir_all(&h).unwrap();
}

#[test]
fn purge_named_plan_reports_items_waiting_for_the_age_threshold() {
    let h = tmp("purge-young-plan");
    let id = "20990101-abcdef";
    let plan = h.join("agentdisk/quarantine").join(id);
    let first = plan.join("000-node_modules");
    let second = plan.join("001-target");
    fs::create_dir_all(&first).unwrap();
    fs::create_dir_all(&second).unwrap();
    write_journal_at(
        &plan,
        &[
            (&h.join("app/node_modules"), &first),
            (&h.join("app/target"), &second),
        ],
        agentdisk_core::size::now_secs(),
    );
    let journal = fs::read(plan.join("journal.json")).unwrap();
    let (code, out, err) = run(&h, &["purge", "--plan", id, "--dry-run"]);
    assert_eq!(code, 0, "{out}{err}");
    assert_eq!(out, format!("Plan {id} has 2 item(s) in quarantine, none older than 7 day(s). Use --older-than 0 to delete them now.\n"));
    assert_eq!(err, "");
    let (code, out, err) = run(&h, &["purge", "--dry-run"]);
    assert_eq!(code, 0, "{out}{err}");
    assert_eq!(out, "Nothing in quarantine is older than 7 day(s).\n");
    assert_eq!(err, "");
    assert_eq!(fs::read(plan.join("journal.json")).unwrap(), journal);
    assert!(first.is_dir() && second.is_dir());
    fs::remove_dir_all(&h).unwrap();
}

#[test]
fn purge_huge_age_leaves_fresh_quarantine_entries_alone() {
    let h = tmp("purge-huge-age");
    let plan = h.join("agentdisk/quarantine/20990101-abcdef");
    let inside = plan.join("000-node_modules");
    fs::create_dir_all(&inside).unwrap();
    fs::write(inside.join("keep.txt"), b"keep").unwrap();
    write_journal_at(
        &plan,
        &[(&h.join("app/node_modules"), &inside)],
        agentdisk_core::size::now_secs(),
    );
    let journal = fs::read(plan.join("journal.json")).unwrap();

    for days in [7, 1 << 57, u64::MAX] {
        for selected_plan in [false, true] {
            let days = days.to_string();
            let mut args = vec!["purge", "--older-than", &days, "--dry-run"];
            if selected_plan {
                args.extend(["--plan", "20990101-abcdef"]);
            }
            let (code, out, err) = run(&h, &args);
            assert_eq!(code, 0, "{out}{err}");
            assert_eq!(
                out,
                if selected_plan {
                    format!("Plan 20990101-abcdef has 1 item(s) in quarantine, none older than {days} day(s). Use --older-than 0 to delete them now.\n")
                } else {
                    format!("Nothing in quarantine is older than {days} day(s).\n")
                }
            );
            assert_eq!(err, "");
        }
    }
    let (code, out, err) = run(&h, &["purge", "--older-than", "0", "--dry-run"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.starts_with("1 item(s), "), "{out}");
    assert_eq!(fs::read(plan.join("journal.json")).unwrap(), journal);
    assert_eq!(fs::read(inside.join("keep.txt")).unwrap(), b"keep");
    fs::remove_dir_all(&h).unwrap();
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

fn configured_check_home(tag: &str) -> PathBuf {
    let h = tmp(tag);
    let adapters = h.join("extra-adapters");
    fs::create_dir_all(&adapters).unwrap();
    fs::write(
        adapters.join("check-fixture.toml"),
        r#"id = "check-fixture"
name = "Check Fixture"
vendor = "fixture"
[[path]]
id = "worktrees"
path = "{home}/.check-fixture/worktrees/*"
kind = "worktree"
class = "user-data"
marker = "/.check-fixture/worktrees/"
"#,
    )
    .unwrap();
    for name in ["one", "two"] {
        fs::create_dir_all(h.join(".check-fixture/worktrees").join(name)).unwrap();
    }
    fs::create_dir_all(h.join("agentdisk")).unwrap();
    fs::write(
        h.join("agentdisk/config.toml"),
        format!(
            "adapters = ['{}']\nmin_free_gb = 1000000\nmax_worktrees = 1\n",
            adapters.display()
        ),
    )
    .unwrap();
    h
}

#[test]
fn check_uses_configured_adapters_and_thresholds_with_flag_overrides() {
    let h = configured_check_home("check-config");
    let (code, out, err) = run(&h, &["check", "--json"]);
    assert_eq!(code, 0, "{out}{err}");
    let health = agentdisk_core::json::parse(&out).unwrap();
    assert_eq!(
        health
            .get("agent_worktrees")
            .unwrap()
            .get("Check Fixture")
            .and_then(|v| v.as_u64()),
        Some(2),
        "{out}"
    );
    let warnings = health.get("warnings").unwrap().as_arr().unwrap();
    assert!(
        warnings
            .iter()
            .any(|w| w.as_str().unwrap().contains("2 agent worktrees")),
        "{out}"
    );
    if health.get("free_bytes").unwrap().as_u64().is_some() {
        assert!(
            warnings
                .iter()
                .any(|w| w.as_str().unwrap().contains("Low disk space")),
            "{out}"
        );
    }
    assert!(err.is_empty(), "{err}");

    let flags = ["check", "--min-free-gb", "0", "--max-worktrees", "2"];
    let mut args = flags.to_vec();
    args.push("--json");
    let (code, out, err) = run(&h, &args);
    assert_eq!(code, 0, "{out}{err}");
    let health = agentdisk_core::json::parse(&out).unwrap();
    assert!(
        health.get("warnings").unwrap().as_arr().unwrap().is_empty(),
        "{out}"
    );
    args.pop();
    args.push("--quiet");
    let (code, out, err) = run(&h, &args);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.is_empty() && err.is_empty(), "{out}{err}");
    assert_eq!(
        fs::read_dir(h.join("agentdisk")).unwrap().count(),
        1,
        "check must not write"
    );
    fs::remove_dir_all(&h).unwrap();
}

#[test]
fn mcp_check_uses_configured_adapters_and_thresholds() {
    use std::io::Write;
    let h = configured_check_home("mcp-check-config");
    let mut child = bin(&h)
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.as_mut().unwrap().write_all(concat!(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#,
        "\n",
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"agentdisk_check","arguments":{}}}"#,
        "\n"
    ).as_bytes()).unwrap();
    drop(child.stdin.take());
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{:?}", output);
    let text = String::from_utf8(output.stdout).unwrap();
    let response = agentdisk_core::json::parse(text.lines().last().unwrap()).unwrap();
    let result = response.get("result").unwrap();
    assert_ne!(
        result.get("isError").and_then(|v| v.as_bool()),
        Some(true),
        "{text}"
    );
    let content = result.get("content").unwrap().as_arr().unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();
    assert!(content.contains("2 agent worktrees"), "{text}");
    let (_, health, _) = run(&h, &["check", "--json"]);
    if agentdisk_core::json::parse(&health)
        .unwrap()
        .get("free_bytes")
        .unwrap()
        .as_u64()
        .is_some()
    {
        assert!(content.contains("Low disk space"), "{text}");
    }
    assert_eq!(
        fs::read_dir(h.join("agentdisk")).unwrap().count(),
        1,
        "MCP check must not write"
    );
    fs::remove_dir_all(&h).unwrap();
}

#[test]
fn config_init_includes_check_thresholds() {
    let h = tmp("check-config-init");
    let (code, out, err) = run(&h, &["config", "--init"]);
    assert_eq!(code, 0, "{out}{err}");
    let config = fs::read_to_string(h.join("agentdisk/config.toml")).unwrap();
    assert!(config.contains("min_free_gb = 20"), "{config}");
    assert!(config.contains("max_worktrees = 20"), "{config}");
    fs::remove_dir_all(&h).unwrap();
}

#[test]
fn mcp_invalid_arguments_are_tool_errors_before_planning() {
    use agentdisk_core::json::{self, Value};
    use std::io::Write;

    let h = tmp("mcp-arguments");
    let cases = [
        ("agentdisk_scan", "projects", "demo".into()),
        (
            "agentdisk_advise",
            "projects",
            Value::Arr(vec!["demo".into(), 5u64.into()]),
        ),
        ("agentdisk_plan", "build_days", "0".into()),
        ("agentdisk_plan", "worktree_days", Value::Num(0.5)),
    ];
    let requests = cases
        .iter()
        .enumerate()
        .map(|(id, (tool, arg, value))| {
            Value::obj(vec![
                ("jsonrpc", "2.0".into()),
                ("id", id.into()),
                ("method", "tools/call".into()),
                (
                    "params",
                    Value::obj(vec![
                        ("name", (*tool).into()),
                        ("arguments", Value::obj(vec![(arg, value.clone())])),
                    ]),
                ),
            ])
        })
        .collect();
    let mut child = bin(&h)
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    writeln!(child.stdin.as_mut().unwrap(), "{}", Value::Arr(requests)).unwrap();
    drop(child.stdin.take());
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let response = json::parse(std::str::from_utf8(&output.stdout).unwrap()).unwrap();
    let replies = response.as_arr().unwrap();
    assert_eq!(replies.len(), cases.len());
    for (reply, (_, name, _)) in replies.iter().zip(cases) {
        let result = reply.get("result").unwrap();
        assert_eq!(result.get("isError").unwrap().as_bool(), Some(true));
        let text = result.get("content").unwrap().as_arr().unwrap()[0]
            .get("text")
            .unwrap()
            .as_str()
            .unwrap();
        assert!(text.contains(name), "{text}");
    }
    assert!(
        !h.join("agentdisk").exists(),
        "invalid calls must not save a plan"
    );
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

fn cache_fixture(tag: &str, nested: bool) -> (PathBuf, PathBuf) {
    let h = tmp(tag);
    let adapters = h.join("agentdisk/adapters");
    fs::create_dir_all(&adapters).unwrap();
    let mut adapter = String::from(
        r#"id = "cache-fixture"
name = "Cache Fixture"
[[path]]
id = "cache"
path = "{home}/.cache-fixture/cache"
class = "regenerable"
"#,
    );
    if nested {
        adapter.push_str(
            r#"
[[path]]
id = "data"
path = "{home}/.cache-fixture"
class = "user-data"
"#,
        );
    }
    fs::write(adapters.join("cache-fixture.toml"), adapter).unwrap();
    fs::write(h.join("agentdisk/config.toml"), "worktree_days = 0\n").unwrap();
    let cache = h.join(".cache-fixture/cache");
    fs::create_dir_all(&cache).unwrap();
    (h, cache)
}

fn cache_explanation_and_plan(
    h: &PathBuf,
    cache: &Path,
) -> (agentdisk_core::json::Value, agentdisk_core::json::Value) {
    let (code, out, err) = run(h, &["explain", cache.to_str().unwrap(), "--json"]);
    assert_eq!(code, 0, "{out}{err}");
    let explanation = agentdisk_core::json::parse(&out).unwrap();
    assert!(
        !h.join("agentdisk/plans").exists(),
        "explain must not write"
    );
    let (code, out, err) = run(h, &["plan", "--json"]);
    assert_eq!(code, 0, "{out}{err}");
    (explanation, agentdisk_core::json::parse(&out).unwrap())
}

#[test]
fn explain_keeps_nested_agent_cache_like_plan() {
    let (h, cache) = cache_fixture("nested-cache", true);
    fs::write(cache.join("log"), vec![b'x'; 4096]).unwrap();
    let (explanation, plan) = cache_explanation_and_plan(&h, &cache);
    assert!(
        plan.get("items").unwrap().as_arr().unwrap().is_empty(),
        "{plan}"
    );
    assert!(
        plan.get("skipped").unwrap().as_arr().unwrap().is_empty(),
        "nested paths stay excluded: {plan}"
    );
    assert_eq!(
        explanation.get("verdict").unwrap().as_str(),
        Some("keep"),
        "{explanation}"
    );
    let outer = fs::canonicalize(h.join(".cache-fixture")).unwrap();
    assert!(
        explanation
            .get("reason")
            .unwrap()
            .as_str()
            .unwrap()
            .contains(&agentdisk_core::paths::display(&outer)),
        "{explanation}"
    );
    assert!(cache.join("log").exists(), "nothing may move");
    fs::remove_dir_all(&h).unwrap();
}

#[test]
fn explain_keeps_empty_agent_cache_like_plan() {
    let (h, cache) = cache_fixture("empty-cache", false);
    let (explanation, plan) = cache_explanation_and_plan(&h, &cache);
    assert!(
        plan.get("items").unwrap().as_arr().unwrap().is_empty(),
        "{plan}"
    );
    let skipped = plan.get("skipped").unwrap().as_arr().unwrap();
    assert_eq!(skipped.len(), 1, "{plan}");
    assert_eq!(
        skipped[0].get("reason").unwrap().as_str(),
        Some("nothing to free")
    );
    assert_eq!(
        explanation.get("verdict").unwrap().as_str(),
        Some("keep"),
        "{explanation}"
    );
    assert_eq!(
        explanation.get("reason").unwrap().as_str(),
        Some("nothing to free")
    );
    fs::remove_dir_all(&h).unwrap();
}

#[test]
fn explain_keeps_shared_agent_cache_like_plan() {
    let (h, cache) = cache_fixture("shared-cache", false);
    let outside = h.join("shared-log");
    fs::write(&outside, vec![b'x'; 4096]).unwrap();
    fs::hard_link(&outside, cache.join("log")).unwrap();
    let (explanation, plan) = cache_explanation_and_plan(&h, &cache);
    assert_eq!(
        explanation.get("reclaimable_bytes").unwrap().as_u64(),
        Some(0)
    );
    assert!(
        plan.get("items").unwrap().as_arr().unwrap().is_empty(),
        "{plan}"
    );
    assert_eq!(
        explanation.get("verdict").unwrap().as_str(),
        Some("keep"),
        "{explanation}"
    );
    assert_eq!(
        explanation.get("reason").unwrap().as_str(),
        Some("nothing to free")
    );
    assert!(outside.exists() && cache.join("log").exists());
    fs::remove_dir_all(&h).unwrap();
}

#[test]
fn explain_includes_ordinary_agent_cache_like_plan() {
    let (h, cache) = cache_fixture("ordinary-cache", false);
    fs::write(cache.join("log"), vec![b'x'; 4096]).unwrap();
    let (explanation, plan) = cache_explanation_and_plan(&h, &cache);
    let items = plan.get("items").unwrap().as_arr().unwrap();
    assert_eq!(items.len(), 1, "{plan}");
    assert_eq!(
        items[0].get("reason").unwrap().as_str(),
        Some("regenerable cache, idle today")
    );
    assert!(
        plan.get("skipped").unwrap().as_arr().unwrap().is_empty(),
        "{plan}"
    );
    assert_eq!(
        explanation.get("verdict").unwrap().as_str(),
        Some("cleanable"),
        "{explanation}"
    );
    assert!(cache.join("log").exists(), "a plan must not move anything");
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

#[test]
fn extra_arguments_are_refused_before_scanning_or_writing() {
    let h = tmp("extra-arguments");
    for command in ["scan", "advise", "plan"] {
        let (code, out, err) = run(&h, &[command, "demo"]);
        assert_eq!(code, 2, "{command}: {out}{err}");
        assert_eq!(out, "");
        assert_eq!(
            err,
            "error: unexpected argument 'demo'. Did you mean `-p demo`?\n"
        );
        assert!(
            !h.join("agentdisk").exists(),
            "invalid arguments must not save a plan"
        );
    }
    for command in ["check", "plans", "adapters", "config"] {
        let (code, out, err) = run(&h, &[command, "demo"]);
        assert_eq!(code, 2, "{command}: {out}{err}");
        assert_eq!(out, "");
        assert_eq!(err, "error: unexpected argument 'demo'\n");
    }
    let (code, out, err) = run(&h, &["config", "--init", "demo"]);
    assert_eq!(code, 2, "{out}{err}");
    assert_eq!(out, "");
    assert_eq!(err, "error: unexpected argument 'demo'\n");
    assert!(
        !h.join("agentdisk").exists(),
        "invalid arguments must not write config"
    );
    fs::remove_dir_all(&h).unwrap();
}

#[test]
fn extra_purge_argument_is_not_treated_as_an_all_plan_request() {
    let h = tmp("extra-purge-argument");
    let plan = h.join("agentdisk/quarantine/20990101-abcdef");
    let inside = plan.join("000-node_modules");
    fs::create_dir_all(&inside).unwrap();
    fs::write(inside.join("keep.txt"), b"keep").unwrap();
    write_journal(&plan, &[(&h.join("app/node_modules"), &inside)]);
    let journal = fs::read(plan.join("journal.json")).unwrap();
    let (code, out, err) = run(
        &h,
        &["purge", "20990101-999999", "--older-than", "0", "--dry-run"],
    );
    assert_eq!(code, 2, "{out}{err}");
    assert_eq!(out, "");
    assert_eq!(
        err,
        "error: unexpected argument '20990101-999999'. Did you mean `--plan 20990101-999999`?\n"
    );
    assert_eq!(fs::read(plan.join("journal.json")).unwrap(), journal);
    assert_eq!(fs::read(inside.join("keep.txt")).unwrap(), b"keep");
    fs::remove_dir_all(&h).unwrap();
}

#[test]
fn plan_reports_scan_warnings_in_text_and_json() {
    let h = tmp("plan-warnings");
    let missing = h.join("missing");
    let expected = format!(
        "project folder not found: {}",
        agentdisk_core::paths::display(&missing)
    );
    let (code, out, err) = run(
        &h,
        &["plan", "-p", missing.to_str().unwrap(), "--no-agents"],
    );
    assert_eq!(code, 0, "{out}{err}");
    assert!(
        out.contains("Nothing to clean, but the scan had warnings."),
        "{out}"
    );
    assert!(out.contains(&format!("WARNINGS\n  - {expected}")), "{out}");

    let (code, out, err) = run(
        &h,
        &[
            "plan",
            "-p",
            missing.to_str().unwrap(),
            "--no-agents",
            "--json",
        ],
    );
    assert_eq!(code, 0, "{out}{err}");
    let plan = agentdisk_core::json::parse(&out).unwrap();
    assert_eq!(plan.get("schema_version").unwrap().as_u64(), Some(1));
    assert_eq!(
        plan.get("warnings").unwrap().as_arr().unwrap(),
        &[agentdisk_core::json::Value::from(expected)]
    );
    assert!(plan.get("items").unwrap().as_arr().unwrap().is_empty());
    let id = plan.get("id").unwrap().as_str().unwrap();
    let saved = fs::read_to_string(h.join("agentdisk/plans").join(format!("{id}.json"))).unwrap();
    assert_eq!(agentdisk_core::json::parse(&saved).unwrap(), plan);
    assert!(
        !missing.exists(),
        "planning must not create the missing project root"
    );
    fs::remove_dir_all(&h).unwrap();
}

#[test]
fn plan_without_scan_warnings_keeps_its_empty_message() {
    let h = tmp("plan-no-warnings");
    let project = h.join("project");
    fs::create_dir_all(&project).unwrap();
    let (code, out, err) = run(
        &h,
        &["plan", "-p", project.to_str().unwrap(), "--no-agents"],
    );
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("Nothing is safe to clean right now."), "{out}");
    assert!(!out.contains("WARNINGS"), "{out}");
    fs::remove_dir_all(&h).unwrap();
}
