//! End-to-end: real git repo + worktrees → scan → plan → apply → restore →
//! apply → purge. One test function, because it sets process env vars.

use agentdisk_core::cleanup::{self, ItemKind, PlanOptions};
use agentdisk_core::scan::ScanOptions;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn tmp() -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let d = std::env::temp_dir().join(format!("agentdisk-flow-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&d).unwrap();
    agentdisk_core::paths::plain(&agentdisk_core::paths::canon(&d))
}

#[test]
fn full_cleanup_flow() {
    if Command::new("git").arg("--version").output().is_err() {
        eprintln!("git not installed; skipping");
        return;
    }
    let root = tmp();
    let home = root.join("home");
    fs::create_dir_all(&home).unwrap();
    // Isolate everything agentdisk reads or writes.
    std::env::set_var("HOME", &home);
    std::env::set_var("USERPROFILE", &home);
    std::env::set_var("APPDATA", home.join("AppData/Roaming"));
    std::env::set_var("LOCALAPPDATA", home.join("AppData/Local"));
    std::env::set_var("XDG_DATA_HOME", home.join(".local/share"));
    std::env::set_var("XDG_CONFIG_HOME", home.join(".config"));
    std::env::set_var("XDG_CACHE_HOME", home.join(".cache"));
    std::env::set_var("AGENTDISK_HOME", root.join("agentdisk-data"));
    std::env::remove_var("CLAUDE_CONFIG_DIR");
    std::env::remove_var("CODEX_HOME");

    // Agent data that must never be planned.
    fs::create_dir_all(home.join(".claude/projects/p1")).unwrap();
    fs::write(home.join(".claude/projects/p1/s.jsonl"), vec![b'x'; 5000]).unwrap();

    // A repo with three Claude worktrees: clean, dirty, and one with a .env.
    let projects = root.join("projects");
    let repo = projects.join("app");
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    fs::write(repo.join("package.json"), "{}").unwrap();
    fs::write(repo.join(".gitignore"), "node_modules/\n.env\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "init"]);
    let wt = |name: &str| {
        let p = repo.join(".claude/worktrees").join(name);
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                &format!("wt-{name}"),
                p.to_str().unwrap(),
            ],
        );
        fs::create_dir_all(p.join("node_modules/dep")).unwrap();
        fs::write(p.join("node_modules/dep/index.js"), vec![b'j'; 20_000]).unwrap();
        p
    };
    let clean = wt("clean");
    let dirty = wt("dirty");
    fs::write(dirty.join("new-file.txt"), "work in progress").unwrap();
    let secret = wt("secret");
    fs::write(secret.join(".env"), "TOKEN=abc").unwrap();
    // A build folder in the main project.
    fs::create_dir_all(repo.join("node_modules/x")).unwrap();
    fs::write(repo.join("node_modules/x/a.js"), vec![b'a'; 30_000]).unwrap();

    let opts = PlanOptions {
        scan: ScanOptions {
            projects: vec![projects.clone()],
            ..ScanOptions::default()
        },
        worktree_days: 0,
        build_days: 0,
        ..PlanOptions::default()
    };
    let (plan, report) = cleanup::plan_now(&opts);
    assert_eq!(report.worktrees.len(), 3, "{:#?}", report.worktrees);
    assert!(report.worktrees.iter().all(|w| w.agent == "claude-code"));

    let planned: Vec<&PathBuf> = plan.items.iter().map(|i| &i.path).collect();
    let skipped: Vec<(String, String)> = plan
        .skipped
        .iter()
        .map(|s| (s.path.display().to_string(), s.reason.clone()))
        .collect();
    let is_planned = |p: &Path| {
        planned
            .iter()
            .any(|x| agentdisk_core::paths::canon(x) == agentdisk_core::paths::canon(p))
    };
    assert!(
        is_planned(&clean),
        "clean worktree should be planned: {skipped:?}"
    );
    assert!(!is_planned(&dirty), "dirty worktree must be skipped");
    assert!(!is_planned(&secret), "worktree with .env must be skipped");
    assert!(skipped.iter().any(|(_, r)| r.contains("uncommitted")));
    assert!(skipped.iter().any(|(_, r)| r.contains(".env")));
    assert!(
        is_planned(&repo.join("node_modules")),
        "project node_modules should be planned: {skipped:?}"
    );
    // node_modules inside the planned clean worktree is not listed twice.
    assert!(!plan
        .items
        .iter()
        .any(|i| i.path.starts_with(&clean) && i.kind == ItemKind::Build));
    // Transcripts are never planned.
    assert!(!plan
        .items
        .iter()
        .any(|i| i.path.to_string_lossy().contains(".claude/projects")));

    cleanup::save(&plan).unwrap();
    let loaded = cleanup::load("latest").unwrap();
    assert_eq!(loaded.id, plan.id);

    // Dry run changes nothing.
    let dry = cleanup::apply(&loaded, true);
    assert!(!dry.moved.is_empty());
    assert!(clean.exists());

    // Apply → quarantine.
    let out = cleanup::apply(&loaded, false);
    assert!(out.skipped.is_empty(), "{:?}", out.skipped);
    assert!(!clean.exists());
    assert!(!repo.join("node_modules").exists());
    assert!(dirty.exists() && secret.exists());
    assert!(git(&repo, &["branch", "--list", "wt-clean"]).contains("wt-clean"));

    // Restore → everything back and git still knows the worktree.
    let (restored, problems) = cleanup::restore(&loaded.id);
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(restored.len(), out.moved.len());
    assert!(clean.join("node_modules/dep/index.js").exists());
    assert_eq!(
        git(&clean, &["rev-parse", "--abbrev-ref", "HEAD"]).trim(),
        "wt-clean"
    );

    // Apply again, then purge for good.
    let out2 = cleanup::apply(&loaded, false);
    assert_eq!(out2.moved.len(), out.moved.len(), "{:?}", out2.skipped);
    let candidates = cleanup::purge_candidates(Some(&loaded.id), 0);
    assert_eq!(candidates.len(), out2.moved.len());
    let (freed, problems) = cleanup::purge(&candidates);
    assert!(problems.is_empty(), "{problems:?}");
    assert!(freed > 0);
    for (_, e) in &candidates {
        assert!(!e.quarantine.exists());
    }
    // Only the purged worktree's record is gone; the others remain; branch kept.
    let list = git(&repo, &["worktree", "list", "--porcelain"]);
    assert!(!list.contains("worktrees/clean"));
    assert!(list.contains("worktrees/dirty") && list.contains("worktrees/secret"));
    assert!(git(&repo, &["branch", "--list", "wt-clean"]).contains("wt-clean"));
    // Transcripts untouched.
    assert!(home.join(".claude/projects/p1/s.jsonl").exists());

    let _ = fs::remove_dir_all(&root);
}
