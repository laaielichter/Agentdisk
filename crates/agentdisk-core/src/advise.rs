//! `agentdisk advise`: ways to need less space, beyond deleting.
//! Duplicates matter more than compression, so most advice is about sharing
//! one copy of dependencies between worktrees.

use crate::json::Value;
use crate::paths::{self, display};
use crate::scan::Report;
use crate::size::{self, human};
use crate::sys;
use crate::util;
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Advice {
    pub title: String,
    pub detail: String,
    /// Rough bytes this could save (when known).
    pub saving: Option<u64>,
    pub commands: Vec<String>,
}

struct Cache {
    name: &'static str,
    path: PathBuf,
    cleanup: &'static str,
}

fn known_caches() -> Vec<Cache> {
    let h = paths::home();
    let win = cfg!(windows);
    let mac = cfg!(target_os = "macos");
    let local = paths::localappdata();
    let env = |v: &str| {
        std::env::var_os(v)
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
    };
    vec![
        Cache {
            name: "npm cache",
            path: env("npm_config_cache").unwrap_or_else(|| {
                if win {
                    local.join("npm-cache")
                } else {
                    h.join(".npm")
                }
            }),
            cleanup: "npm cache clean --force",
        },
        Cache {
            name: "pnpm store",
            path: pnpm_store(),
            cleanup: "pnpm store prune",
        },
        Cache {
            name: "uv cache",
            path: uv_cache(),
            cleanup: "uv cache prune",
        },
        Cache {
            name: "pip cache",
            path: env("PIP_CACHE_DIR").unwrap_or_else(|| {
                if win {
                    local.join("pip").join("cache")
                } else if mac {
                    h.join("Library/Caches/pip")
                } else {
                    paths::xdg_cache().join("pip")
                }
            }),
            cleanup: "pip cache purge",
        },
        Cache {
            name: "Cargo registry",
            path: env("CARGO_HOME")
                .unwrap_or_else(|| h.join(".cargo"))
                .join("registry"),
            cleanup: "automatic since Rust 1.88 (unused downloads are removed after 3 months)",
        },
        Cache {
            name: "Go module cache",
            path: env("GOMODCACHE").unwrap_or_else(|| {
                env("GOPATH")
                    .unwrap_or_else(|| h.join("go"))
                    .join("pkg/mod")
            }),
            cleanup: "go clean -modcache",
        },
        Cache {
            name: "Gradle caches",
            path: env("GRADLE_USER_HOME")
                .unwrap_or_else(|| h.join(".gradle"))
                .join("caches"),
            cleanup: "delete ~/.gradle/caches while no Gradle daemon runs (gradle --stop)",
        },
        Cache {
            name: "Maven repository",
            path: h.join(".m2/repository"),
            cleanup: "re-downloads on next build; delete old versions only",
        },
        Cache {
            name: "NuGet packages",
            path: env("NUGET_PACKAGES").unwrap_or_else(|| h.join(".nuget/packages")),
            cleanup: "dotnet nuget locals all --clear",
        },
    ]
}

pub fn pnpm_store() -> PathBuf {
    if let Some(p) = std::env::var_os("PNPM_STORE_DIR").filter(|s| !s.is_empty()) {
        return PathBuf::from(p);
    }
    if cfg!(windows) {
        paths::localappdata().join("pnpm").join("store")
    } else if cfg!(target_os = "macos") {
        paths::home().join("Library/pnpm/store")
    } else {
        paths::xdg_data().join("pnpm").join("store")
    }
}

pub fn uv_cache() -> PathBuf {
    if let Some(p) = std::env::var_os("UV_CACHE_DIR").filter(|s| !s.is_empty()) {
        return PathBuf::from(p);
    }
    if cfg!(windows) {
        paths::localappdata().join("uv").join("cache")
    } else {
        paths::xdg_cache().join("uv")
    }
}

pub fn advise(r: &Report) -> Vec<Advice> {
    let mut out = Vec::new();
    let threads = size::default_threads();

    // 1. Identical dependency trees (same folder type + same lockfile).
    let mut groups: BTreeMap<(String, String, u64), Vec<(PathBuf, u64)>> = BTreeMap::new();
    for e in &r.eco {
        if !matches!(e.rule.dir, "node_modules" | ".venv" | "venv" | "target") {
            continue;
        }
        const LOCKS: &[&str] = &[
            "pnpm-lock.yaml",
            "package-lock.json",
            "yarn.lock",
            "bun.lock",
            "uv.lock",
            "poetry.lock",
            "requirements.txt",
            "Cargo.lock",
        ];
        if let Some((name, hash)) = LOCKS
            .iter()
            .find_map(|n| util::fnv_file(&e.project.join(n)).map(|h| (n.to_string(), h)))
        {
            groups
                .entry((e.rule.dir.to_string(), name, hash))
                .or_default()
                .push((e.path.clone(), e.totals.reclaimable_bytes));
        }
    }
    for ((dir, lock, _), members) in &groups {
        if members.len() < 2 {
            continue;
        }
        let total: u64 = members.iter().map(|m| m.1).sum();
        let largest = members.iter().map(|m| m.1).max().unwrap_or(0);
        let (fix, cmds): (&str, Vec<String>) = match (dir.as_str(), lock.as_str()) {
            ("node_modules", "pnpm-lock.yaml") => (
                "They already use pnpm, but the copies are not shared. Keep the pnpm store on the same drive as the projects so pnpm can hardlink.",
                vec![format!("pnpm store path   # should be on the same drive as your projects")],
            ),
            ("node_modules", _) => (
                "Switch these projects to pnpm: one shared store, hardlinked into each worktree.",
                vec!["npm install -g pnpm".into(), "pnpm import   # converts the lockfile".into(), "pnpm install".into()],
            ),
            (".venv" | "venv", _) => (
                "Use uv: it links packages from one cache into every environment.",
                vec!["uv venv".into(), "uv pip sync requirements.txt   # or: uv sync".into()],
            ),
            _ => (
                "Rust build output cannot be shared safely between parallel agents (Cargo locks the folder). Remove stale target folders with cargo-sweep, or seed new worktrees with a copy-on-write clone.",
                vec!["cargo install cargo-sweep".into(), "cargo sweep --time 14".into()],
            ),
        };
        out.push(Advice {
            title: format!("{} copies of {dir} with the same {lock}", members.len()),
            detail: format!("{} in total. {fix}", human(total)),
            saving: Some(total.saturating_sub(largest)),
            commands: cmds,
        });
    }

    // 2. npm / pip without sharing at all.
    let npm_projects = r
        .eco
        .iter()
        .filter(|e| e.rule.dir == "node_modules" && e.project.join("package-lock.json").exists())
        .count();
    if npm_projects >= 3 && !out.iter().any(|a| a.title.contains("node_modules")) {
        out.push(Advice {
            title: format!("{npm_projects} npm projects each keep a full node_modules"),
            detail: "pnpm stores every package once and links it into projects. Savings grow with every worktree.".into(),
            saving: None,
            commands: vec!["npm install -g pnpm".into(), "pnpm import && pnpm install".into()],
        });
    }

    // 3. Caches must be on the same drive as projects, or links become copies.
    let project_vols: Vec<String> = r
        .projects
        .iter()
        .filter_map(|p| sys::volume_of(p))
        .collect();
    for (name, cache) in [("pnpm store", pnpm_store()), ("uv cache", uv_cache())] {
        if !cache.exists() {
            continue;
        }
        if let Some(v) = sys::volume_of(&cache) {
            if !project_vols.is_empty() && project_vols.iter().any(|pv| *pv != v) {
                out.push(Advice {
                    title: format!("{name} is on a different drive than some projects"),
                    detail: format!(
                        "{} lives at {}. Across drives, packages are copied instead of linked, which doubles disk use.",
                        name,
                        display(&cache)
                    ),
                    saving: None,
                    commands: vec![if name == "uv cache" {
                        "set UV_CACHE_DIR to a folder on the projects' drive".into()
                    } else {
                        "pnpm config set store-dir <folder on the projects' drive>".into()
                    }],
                });
            }
        }
    }

    // 4. Big package caches with their own cleanup command.
    for c in known_caches() {
        if !c.path.exists() {
            continue;
        }
        let Ok(t) = size::measure(&c.path, threads) else {
            continue;
        };
        if t.allocated_bytes < 1024 * 1024 * 1024 {
            continue;
        }
        out.push(Advice {
            title: format!("{} uses {}", c.name, human(t.allocated_bytes)),
            detail: format!(
                "Location: {}. Use the tool's own cleanup; it knows what is still needed.",
                display(&c.path)
            ),
            saving: None,
            commands: vec![c.cleanup.to_string()],
        });
    }

    // 5. Stale worktree records.
    let mut prune_repos: Vec<String> = r
        .worktrees
        .iter()
        .filter(|w| w.prunable)
        .filter_map(|w| w.repo.as_ref().map(|p| display(p)))
        .collect();
    prune_repos.sort();
    prune_repos.dedup();
    if !prune_repos.is_empty() {
        out.push(Advice {
            title: format!(
                "{} repo(s) list worktrees whose folder is gone",
                prune_repos.len()
            ),
            detail: "Harmless, but the list grows. git can forget them.".into(),
            saving: None,
            commands: prune_repos
                .iter()
                .map(|r| format!("git -C \"{r}\" worktree prune"))
                .collect(),
        });
    }

    // 6. Agent settings that cap growth.
    let present: Vec<&str> = r
        .agents
        .iter()
        .map(|a| a.adapter.as_str())
        .chain(r.worktrees.iter().map(|w| w.agent.as_str()))
        .collect();
    if present.contains(&"claude-code") {
        out.push(Advice {
            title: "Claude Code: choose how long transcripts are kept".into(),
            detail: "cleanupPeriodDays (default 30) decides when old sessions are deleted. Raise it to keep history, lower it to save space. Back up anything you want to keep first.".into(),
            saving: None,
            commands: vec!["~/.claude/settings.json → \"cleanupPeriodDays\": 30".into()],
        });
    }
    if present.contains(&"cursor") {
        out.push(Advice {
            title: "Cursor: cap worktrees".into(),
            detail: "cursor.worktreeMaxCount (25) and cursor.worktreesGlobalMaxSizeGb (50) limit growth. Cursor's cleanup can also remove worktrees made by other tools.".into(),
            saving: None,
            commands: vec!["Cursor settings → \"cursor.worktreesGlobalMaxSizeGb\": 20".into()],
        });
    }

    // 7. Copy-on-write and compression, per platform.
    let wt_total: u64 = r.worktrees.iter().map(|w| w.totals.allocated_bytes).sum();
    if cfg!(windows) {
        out.push(Advice {
            title: "Windows: Dev Drive for projects and caches".into(),
            detail: "A Dev Drive (ReFS, at least 50 GB) is faster for builds and supports block cloning, so copied files share space. Put package caches on it too. Measure first; ReFS cloning has had reported data issues with unflushed writes, so only clone from folders that are not being written.".into(),
            saving: None,
            commands: vec!["Settings → System → Storage → Disks & volumes → Create Dev Drive".into()],
        });
    } else if cfg!(target_os = "macos") {
        out.push(Advice {
            title: "macOS: APFS clones".into(),
            detail: "`cp -c` makes copy-on-write clones on APFS: a new worktree's dependency folder can start as a clone and only uses space for changes.".into(),
            saving: None,
            commands: vec!["cp -cR <worktree-a>/node_modules <worktree-b>/".into()],
        });
    } else {
        out.push(Advice {
            title: "Linux: reflinks and compression".into(),
            detail: "On Btrfs or XFS, `cp --reflink=always` makes free copies. Btrfs with compress=zstd:1 saves about 30-40% on code and build output.".into(),
            saving: None,
            commands: vec!["cp -r --reflink=always <src> <dst>".into(), "mount option: compress=zstd:1".into()],
        });
    }
    if wt_total > 0 {
        out.push(Advice {
            title: "Compression is a small lever".into(),
            detail: "Expect 1.3–2x on dev folders and almost nothing on .git, images, audio and video. Use `agentdisk compact` only on folders that are mostly read (SDKs, old projects, engine installs).".into(),
            saving: None,
            commands: vec!["agentdisk compact <folder>".into()],
        });
    }
    out
}

pub fn render(list: &[Advice]) -> String {
    let mut out = String::from("agentdisk advise — ways to need less space\n\n");
    if list.is_empty() {
        out.push_str("No advice: nothing stood out.\n");
    }
    for (i, a) in list.iter().enumerate() {
        out.push_str(&format!("{}. {}", i + 1, a.title));
        if let Some(s) = a.saving.filter(|s| *s > 0) {
            out.push_str(&format!("  (could save ~{})", human(s)));
        }
        out.push('\n');
        out.push_str(&format!("   {}\n", a.detail));
        for c in &a.commands {
            out.push_str(&format!("   $ {c}\n"));
        }
        out.push('\n');
    }
    out.push_str("agentdisk never runs these for you; they are suggestions.\n");
    out
}

pub fn to_json(list: &[Advice]) -> Value {
    Value::Arr(
        list.iter()
            .map(|a| {
                Value::obj(vec![
                    ("title", a.title.as_str().into()),
                    ("detail", a.detail.as_str().into()),
                    ("saving_bytes", a.saving.into()),
                    (
                        "commands",
                        a.commands
                            .iter()
                            .map(|c| Value::from(c.as_str()))
                            .collect::<Vec<_>>()
                            .into(),
                    ),
                ])
            })
            .collect(),
    )
}
