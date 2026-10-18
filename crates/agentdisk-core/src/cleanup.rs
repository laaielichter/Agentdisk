//! Safe cleanup: `plan` → `apply` → (`restore` | `purge`).
//!
//! - `plan` only writes a plan file. It lists what would move and why, and
//!   everything it skipped and why.
//! - `apply` re-checks every item, then *moves* it into a quarantine folder
//!   (same volume, instant) and records it in a journal. Nothing is deleted.
//! - `restore` moves items back. `purge` deletes quarantined items after a
//!   waiting period.
//!
//! Safety rules (cannot be switched off):
//! - Worktrees: never the main one, never locked, never with uncommitted or
//!   untracked files, never with ignored files that are not build output
//!   (like `.env`), never with commits that are on no branch or tag.
//!   The branch itself is always kept.
//! - Build folders: only with their marker file, only when git-ignored and
//!   holding no tracked files, never while their editor runs.
//! - Agent data: only folders an adapter marks `regenerable`. Transcripts,
//!   checkpoints, memories, settings and models are never planned.
//! - Anything changed after the plan was made is skipped at apply time.

use crate::adapters::Class;
use crate::git;
use crate::json::{self, Value};
use crate::paths::{self, display};
use crate::projects;
use crate::scan::{self, Report, ScanOptions, WorktreeItem};
use crate::size::{self, human};
use crate::sys;
use crate::util::{self, table};
use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    Worktree,
    Build,
    AgentCache,
}

impl ItemKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ItemKind::Worktree => "worktree",
            ItemKind::Build => "build-folder",
            ItemKind::AgentCache => "agent-cache",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "worktree" => ItemKind::Worktree,
            "build-folder" => ItemKind::Build,
            "agent-cache" => ItemKind::AgentCache,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone)]
pub struct PlanItem {
    pub kind: ItemKind,
    pub path: PathBuf,
    pub repo: Option<PathBuf>,
    pub agent: String,
    pub label: String,
    pub reason: String,
    pub bytes: u64,
    pub newest_mtime: u64,
}

#[derive(Debug, Clone)]
pub struct Skipped {
    pub path: PathBuf,
    pub label: String,
    pub reason: String,
    pub bytes: u64,
}

#[derive(Debug, Clone)]
pub struct Plan {
    pub id: String,
    pub created: u64,
    pub items: Vec<PlanItem>,
    pub skipped: Vec<Skipped>,
    pub hints: Vec<String>,
}

impl Plan {
    /// What a human types to confirm `apply`.
    pub fn code(&self) -> String {
        self.id
            .chars()
            .rev()
            .take(6)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    }
    pub fn total(&self) -> u64 {
        self.items.iter().map(|i| i.bytes).sum()
    }
}

#[derive(Debug, Clone)]
pub struct PlanOptions {
    pub scan: ScanOptions,
    /// Minimum days without changes before a worktree is planned.
    pub worktree_days: u64,
    /// Minimum days without changes before a build folder is planned.
    pub build_days: u64,
    pub worktrees: bool,
    pub build: bool,
    pub agent_caches: bool,
}

impl Default for PlanOptions {
    fn default() -> Self {
        PlanOptions {
            scan: ScanOptions::default(),
            worktree_days: 7,
            build_days: 14,
            worktrees: true,
            build: true,
            agent_caches: true,
        }
    }
}

/// Ignored entries that may be lost with a worktree: build output only.
fn ignored_is_regenerable(entry: &str) -> bool {
    let e = entry.trim_end_matches('/');
    let last = e.rsplit('/').next().unwrap_or(e);
    if last.ends_with(".log") || last.ends_with(".pyc") || last.ends_with(".tsbuildinfo") {
        return true;
    }
    const EXTRA: &[&str] = &[
        "__pycache__",
        "dist",
        "out",
        "coverage",
        ".cache",
        ".eslintcache",
        ".DS_Store",
        "Thumbs.db",
        ".pnpm-store",
    ];
    e.split('/')
        .any(|c| EXTRA.contains(&c) || projects::RULES.iter().any(|r| r.dir == c))
}

fn running_editor(procs: &[String], editor: &str) -> bool {
    procs.iter().any(|p| p.contains(editor))
}

/// Check one worktree. `Ok(reason)` = safe to plan, `Err(reason)` = skip.
pub fn check_worktree(w: &WorktreeItem, now: u64, min_days: u64) -> Result<String, String> {
    if w.orphan {
        return Err("its repository was not found; check it by hand".into());
    }
    if w.prunable {
        return Err("folder is missing; run `git worktree prune` in the repo".into());
    }
    if w.locked {
        return Err("locked (an agent may be using it)".into());
    }
    match w.dirty {
        None => return Err("could not read git status".into()),
        Some(0) => {}
        Some(n) => return Err(format!("{n} uncommitted or untracked file(s)")),
    }
    let idle = w.totals.idle_days(now).unwrap_or(0);
    if idle < min_days {
        return Err(format!(
            "changed {} — needs {min_days}+ days idle",
            util::age(Some(idle))
        ));
    }
    let Some(ignored) = git::ignored_entries(&w.path) else {
        return Err("could not list ignored files".into());
    };
    let risky: Vec<&String> = ignored
        .iter()
        .filter(|e| !ignored_is_regenerable(e))
        .collect();
    if !risky.is_empty() {
        let list: Vec<&str> = risky.iter().take(3).map(|s| s.as_str()).collect();
        return Err(format!(
            "has ignored files that are not build output: {}",
            list.join(", ")
        ));
    }
    let branch_note = match (&w.branch, &w.head, &w.repo) {
        (Some(b), _, _) => format!("branch '{b}' is kept"),
        (None, Some(h), Some(repo)) => {
            if git::reachable(repo, h) {
                "detached, commit is on a branch".to_string()
            } else {
                return Err("detached HEAD with commits on no branch".into());
            }
        }
        _ => return Err("unknown HEAD".into()),
    };
    let users = sys::processes_using(&w.path);
    if !users.is_empty() {
        return Err(format!("in use by {}", users.join(", ")));
    }
    Ok(format!(
        "clean, idle {}, {branch_note}",
        util::age(Some(idle))
    ))
}

fn check_build(
    path: &Path,
    idle: Option<u64>,
    reclaim: u64,
    min_days: u64,
    procs: &[String],
) -> Result<String, String> {
    let Some(rule) = projects::rule_for(path) else {
        return Err("marker file is gone".into());
    };
    let idle = idle.unwrap_or(0);
    if idle < min_days {
        return Err(format!(
            "changed {} — needs {min_days}+ days idle",
            util::age(Some(idle))
        ));
    }
    match git::is_ignored(path) {
        Some(false) => return Err("not ignored by git".into()),
        Some(true) if git::has_tracked(path) => return Err("contains files tracked by git".into()),
        _ => {}
    }
    if let Some(ed) = rule.editor {
        if running_editor(procs, ed) {
            return Err(format!("close the {ed} editor first"));
        }
    }
    if reclaim == 0 {
        return Err("all its files are shared (hardlinks); deleting frees nothing".into());
    }
    let users = sys::processes_using(path);
    if !users.is_empty() {
        return Err(format!("in use by {}", users.join(", ")));
    }
    Ok(format!(
        "{} output, idle {}, rebuild: {}",
        rule.ecosystem,
        util::age(Some(idle)),
        rule.rebuild
    ))
}

pub fn make_plan(report: &Report, opts: &PlanOptions) -> Plan {
    let now = report.generated;
    let procs = sys::process_names();
    let mut items = Vec::new();
    let mut skipped = Vec::new();
    let mut hints = Vec::new();
    let mut planned_wt: Vec<String> = Vec::new();

    if opts.worktrees {
        for w in &report.worktrees {
            let label = format!(
                "{} worktree {}",
                w.agent,
                w.branch.as_deref().unwrap_or("(detached)")
            );
            match check_worktree(w, now, opts.worktree_days) {
                Ok(reason) => {
                    planned_wt.push(paths::slashed(&w.path));
                    items.push(PlanItem {
                        kind: ItemKind::Worktree,
                        path: w.path.clone(),
                        repo: w.repo.clone(),
                        agent: w.agent.clone(),
                        label,
                        reason,
                        bytes: w.totals.reclaimable_bytes,
                        newest_mtime: w.totals.newest_mtime,
                    })
                }
                Err(reason) => {
                    if w.prunable {
                        if let Some(r) = &w.repo {
                            hints.push(format!("git -C \"{}\" worktree prune", display(r)));
                        }
                    }
                    skipped.push(Skipped {
                        path: w.path.clone(),
                        label,
                        reason,
                        bytes: w.totals.reclaimable_bytes,
                    })
                }
            }
        }
    }

    if opts.build {
        for e in &report.eco {
            let s = paths::slashed(&e.path);
            if planned_wt.iter().any(|w| s.starts_with(&format!("{w}/"))) {
                continue; // removed together with its worktree
            }
            let label = format!("{} ({})", e.rule.dir, e.rule.ecosystem);
            match check_build(
                &e.path,
                e.totals.idle_days(now),
                e.totals.reclaimable_bytes,
                opts.build_days,
                &procs,
            ) {
                Ok(reason) => items.push(PlanItem {
                    kind: ItemKind::Build,
                    path: e.path.clone(),
                    repo: None,
                    agent: if e.worktree.is_some() {
                        "worktree".into()
                    } else {
                        "project".into()
                    },
                    label,
                    reason,
                    bytes: e.totals.reclaimable_bytes,
                    newest_mtime: e.totals.newest_mtime,
                }),
                Err(reason) => skipped.push(Skipped {
                    path: e.path.clone(),
                    label,
                    reason,
                    bytes: e.totals.reclaimable_bytes,
                }),
            }
        }
    }

    if opts.agent_caches {
        for a in report
            .agents
            .iter()
            .filter(|a| !a.nested && a.class == Class::Regenerable)
        {
            let label = format!("{} {}", a.agent, a.what);
            let adapter = report.adapters.iter().find(|x| x.id == a.adapter);
            let running = adapter.is_some_and(|ad| {
                ad.processes
                    .iter()
                    .any(|p| procs.iter().any(|q| q == &p.to_lowercase()))
            });
            let idle = a.totals.idle_days(now).unwrap_or(0);
            let reason = if running {
                Err(format!("{} is running", a.agent))
            } else if idle < opts.worktree_days {
                Err(format!(
                    "changed {} — needs {}+ days idle",
                    util::age(Some(idle)),
                    opts.worktree_days
                ))
            } else if a.totals.reclaimable_bytes == 0 {
                Err("nothing to free".to_string())
            } else {
                Ok(format!(
                    "regenerable {}, idle {}",
                    a.what,
                    util::age(Some(idle))
                ))
            };
            match reason {
                Ok(reason) => items.push(PlanItem {
                    kind: ItemKind::AgentCache,
                    path: a.path.clone(),
                    repo: None,
                    agent: a.adapter.clone(),
                    label,
                    reason,
                    bytes: a.totals.reclaimable_bytes,
                    newest_mtime: a.totals.newest_mtime,
                }),
                Err(reason) => skipped.push(Skipped {
                    path: a.path.clone(),
                    label,
                    reason,
                    bytes: a.totals.reclaimable_bytes,
                }),
            }
        }
    }

    // Vendor tools first, where they exist.
    let present: HashSet<&str> = report
        .agents
        .iter()
        .map(|a| a.adapter.as_str())
        .chain(report.worktrees.iter().map(|w| w.agent.as_str()))
        .collect();
    for a in &report.adapters {
        if let (true, Some(c)) = (present.contains(a.id.as_str()), &a.cleanup) {
            hints.push(format!("{}: {c}", a.name));
        }
    }
    hints.sort();
    hints.dedup();

    items.sort_by_key(|i| std::cmp::Reverse(i.bytes));
    skipped.sort_by_key(|s| std::cmp::Reverse(s.bytes));
    Plan {
        id: util::new_id(),
        created: now,
        items,
        skipped,
        hints,
    }
}

// ---------------------------------------------------------------------------
// Persistence

fn plans_dir() -> PathBuf {
    paths::data_dir().join("plans")
}

fn quarantine_dir() -> PathBuf {
    paths::data_dir().join("quarantine")
}

fn write_atomic(path: &Path, text: &str) -> io::Result<()> {
    if let Some(p) = path.parent() {
        fs::create_dir_all(p)?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, text)?;
    fs::rename(&tmp, path)
}

pub fn plan_to_json(p: &Plan) -> Value {
    let items = p
        .items
        .iter()
        .map(|i| {
            Value::obj(vec![
                ("kind", i.kind.as_str().into()),
                ("path", display(&i.path).into()),
                ("repo", i.repo.as_ref().map(|r| display(r)).into()),
                ("agent", i.agent.as_str().into()),
                ("label", i.label.as_str().into()),
                ("reason", i.reason.as_str().into()),
                ("bytes", i.bytes.into()),
                ("newest_mtime", i.newest_mtime.into()),
            ])
        })
        .collect::<Vec<_>>();
    let skipped = p
        .skipped
        .iter()
        .map(|s| {
            Value::obj(vec![
                ("path", display(&s.path).into()),
                ("label", s.label.as_str().into()),
                ("reason", s.reason.as_str().into()),
                ("bytes", s.bytes.into()),
            ])
        })
        .collect::<Vec<_>>();
    Value::obj(vec![
        ("schema_version", 1u64.into()),
        ("id", p.id.as_str().into()),
        ("created", p.created.into()),
        ("total_bytes", p.total().into()),
        ("items", items.into()),
        ("skipped", skipped.into()),
        (
            "hints",
            p.hints
                .iter()
                .map(|h| Value::from(h.as_str()))
                .collect::<Vec<_>>()
                .into(),
        ),
    ])
}

fn plan_from_json(v: &Value) -> Result<Plan, String> {
    let s = |o: &Value, k: &str| o.get(k).and_then(Value::as_str).map(str::to_string);
    let n = |o: &Value, k: &str| o.get(k).and_then(Value::as_u64).unwrap_or(0);
    let mut items = Vec::new();
    for i in v.get("items").and_then(Value::as_arr).unwrap_or(&[]) {
        items.push(PlanItem {
            kind: ItemKind::parse(&s(i, "kind").unwrap_or_default()).ok_or("bad item kind")?,
            path: PathBuf::from(s(i, "path").ok_or("item without path")?),
            repo: s(i, "repo").map(PathBuf::from),
            agent: s(i, "agent").unwrap_or_default(),
            label: s(i, "label").unwrap_or_default(),
            reason: s(i, "reason").unwrap_or_default(),
            bytes: n(i, "bytes"),
            newest_mtime: n(i, "newest_mtime"),
        });
    }
    Ok(Plan {
        id: s(v, "id").ok_or("plan without id")?,
        created: n(v, "created"),
        items,
        skipped: Vec::new(),
        hints: Vec::new(),
    })
}

pub fn save(p: &Plan) -> io::Result<PathBuf> {
    let path = plans_dir().join(format!("{}.json", p.id));
    write_atomic(&path, &plan_to_json(p).pretty())?;
    Ok(path)
}

/// Load a plan by id, or the newest plan for `"latest"`.
pub fn load(id: &str) -> Result<Plan, String> {
    let dir = plans_dir();
    let id = if id == "latest" {
        let mut ids: Vec<String> = fs::read_dir(&dir)
            .map_err(|_| "no plans yet; run `agentdisk plan` first".to_string())?
            .flatten()
            .filter_map(|e| {
                e.file_name()
                    .to_string_lossy()
                    .strip_suffix(".json")
                    .map(str::to_string)
            })
            .collect();
        ids.sort();
        ids.pop()
            .ok_or("no plans yet; run `agentdisk plan` first")?
    } else {
        id.to_string()
    };
    if !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err("bad plan id".into());
    }
    let text = fs::read_to_string(dir.join(format!("{id}.json")))
        .map_err(|_| format!("plan '{id}' not found"))?;
    plan_from_json(&json::parse(&text)?)
}

pub fn render_plan(p: &Plan, path: Option<&Path>) -> String {
    let mut out = format!("agentdisk plan {}\n", p.id);
    out.push_str("This is only a plan. Nothing was moved or deleted.\n\n");
    if p.items.is_empty() {
        out.push_str("Nothing is safe to clean right now.\n");
    } else {
        out.push_str(&format!(
            "WOULD MOVE TO QUARANTINE ({} items, {})\n",
            p.items.len(),
            human(p.total())
        ));
        let rows: Vec<Vec<String>> = p
            .items
            .iter()
            .map(|i| {
                vec![
                    human(i.bytes),
                    i.label.clone(),
                    i.reason.clone(),
                    util::short_path(&display(&i.path), 60),
                ]
            })
            .collect();
        out.push_str(&table(
            &["Frees", "What", "Why it is safe", "Path"],
            &rows,
            &[0],
        ));
    }
    if !p.skipped.is_empty() {
        out.push_str(&format!("\nSKIPPED ({})\n", p.skipped.len()));
        let rows: Vec<Vec<String>> = p
            .skipped
            .iter()
            .take(30)
            .map(|s| {
                vec![
                    human(s.bytes),
                    s.label.clone(),
                    s.reason.clone(),
                    util::short_path(&display(&s.path), 60),
                ]
            })
            .collect();
        out.push_str(&table(
            &["Size", "What", "Why skipped", "Path"],
            &rows,
            &[0],
        ));
        if p.skipped.len() > 30 {
            out.push_str(&format!("  … and {} more\n", p.skipped.len() - 30));
        }
    }
    if !p.hints.is_empty() {
        out.push_str("\nVENDOR CLEANUP COMMANDS (try these first)\n");
        for h in &p.hints {
            out.push_str(&format!("  {h}\n"));
        }
    }
    if let Some(path) = path {
        out.push_str(&format!("\nPlan saved: {}\n", display(path)));
    }
    if !p.items.is_empty() {
        out.push_str(&format!(
            "\nTo apply, a human runs in a terminal:\n  agentdisk apply {}\nand types the code {} to confirm.\nItems go to quarantine first; `agentdisk restore {}` brings them back.\n",
            p.id,
            p.code(),
            p.id
        ));
    }
    out
}

// ---------------------------------------------------------------------------
// Apply / restore / purge

#[derive(Debug, Clone)]
pub struct JournalEntry {
    pub original: PathBuf,
    pub quarantine: PathBuf,
    pub kind: ItemKind,
    pub repo: Option<PathBuf>,
    pub bytes: u64,
    pub moved_at: u64,
    /// "quarantined" | "restored" | "purged"
    pub state: String,
}

fn journal_path(plan_id: &str) -> PathBuf {
    quarantine_dir().join(plan_id).join("journal.json")
}

fn write_journal(plan_id: &str, entries: &[JournalEntry]) -> io::Result<()> {
    let v: Vec<Value> = entries
        .iter()
        .map(|e| {
            Value::obj(vec![
                ("original", display(&e.original).into()),
                ("quarantine", display(&e.quarantine).into()),
                ("kind", e.kind.as_str().into()),
                ("repo", e.repo.as_ref().map(|r| display(r)).into()),
                ("bytes", e.bytes.into()),
                ("moved_at", e.moved_at.into()),
                ("state", e.state.as_str().into()),
            ])
        })
        .collect();
    write_atomic(&journal_path(plan_id), &Value::Arr(v).pretty())
}

pub fn read_journal(plan_id: &str) -> Vec<JournalEntry> {
    let Ok(text) = fs::read_to_string(journal_path(plan_id)) else {
        return Vec::new();
    };
    let Ok(v) = json::parse(&text) else {
        return Vec::new();
    };
    v.as_arr()
        .unwrap_or(&[])
        .iter()
        .filter_map(|e| {
            let s = |k: &str| e.get(k).and_then(Value::as_str).map(str::to_string);
            Some(JournalEntry {
                original: PathBuf::from(s("original")?),
                quarantine: PathBuf::from(s("quarantine")?),
                kind: ItemKind::parse(&s("kind")?)?,
                repo: s("repo").map(PathBuf::from),
                bytes: e.get("bytes").and_then(Value::as_u64).unwrap_or(0),
                moved_at: e.get("moved_at").and_then(Value::as_u64).unwrap_or(0),
                state: s("state").unwrap_or_else(|| "quarantined".into()),
            })
        })
        .collect()
}

/// Plan ids that have a quarantine journal.
pub fn quarantined_plans() -> Vec<String> {
    let mut ids: Vec<String> = fs::read_dir(quarantine_dir())
        .map(|r| {
            r.flatten()
                .filter(|e| e.path().join("journal.json").exists())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    ids.sort();
    ids
}

/// Re-check an item right before moving it.
fn recheck(item: &PlanItem, procs: &[String], threads: usize) -> Result<(), String> {
    if !item.path.exists() {
        return Err("no longer exists".into());
    }
    let t = size::measure(&item.path, threads).map_err(|e| e.to_string())?;
    if t.newest_mtime > item.newest_mtime {
        return Err("changed since the plan was made".into());
    }
    let now = size::now_secs();
    match item.kind {
        ItemKind::Worktree => {
            let repo = item.repo.as_ref().ok_or("worktree without repo")?;
            let key = paths::slashed(&paths::canon(&item.path));
            let w = git::worktrees(repo)
                .into_iter()
                .find(|w| paths::slashed(&paths::canon(&w.path)) == key)
                .ok_or("git no longer lists this worktree")?;
            if w.is_main {
                return Err("this is the main worktree".into());
            }
            let wi = WorktreeItem {
                dirty: git::dirty_files(&w.path).map(|d| d.len()),
                path: w.path,
                repo: Some(repo.clone()),
                agent: item.agent.clone(),
                head: w.head,
                branch: w.branch,
                locked: w.locked,
                prunable: w.prunable,
                orphan: false,
                lockfile: None,
                totals: t,
            };
            check_worktree(&wi, now, 0).map(|_| ())
        }
        ItemKind::Build => check_build(
            &item.path,
            Some(u64::MAX),
            t.reclaimable_bytes.max(1),
            0,
            procs,
        )
        .map(|_| ()),
        ItemKind::AgentCache => Ok(()),
    }
}

#[derive(Debug, Default)]
pub struct ApplyOutcome {
    pub moved: Vec<JournalEntry>,
    pub skipped: Vec<(PathBuf, String)>,
}

/// Move every still-safe item into quarantine. With `dry_run`, only re-check.
pub fn apply(plan: &Plan, dry_run: bool) -> ApplyOutcome {
    let procs = sys::process_names();
    let threads = size::default_threads();
    let mut out = ApplyOutcome::default();
    let mut journal = read_journal(&plan.id);
    let qroot = quarantine_dir().join(&plan.id);

    for (idx, item) in plan.items.iter().enumerate() {
        if journal
            .iter()
            .any(|j| j.original == item.path && j.state == "quarantined")
        {
            out.skipped
                .push((item.path.clone(), "already in quarantine".into()));
            continue;
        }
        if let Err(e) = recheck(item, &procs, threads) {
            out.skipped.push((item.path.clone(), e));
            continue;
        }
        let name = item
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "item".into());
        let entry_name = format!("{idx:03}-{name}");
        if dry_run {
            out.moved.push(JournalEntry {
                original: item.path.clone(),
                quarantine: qroot.join(&entry_name),
                kind: item.kind,
                repo: item.repo.clone(),
                bytes: item.bytes,
                moved_at: size::now_secs(),
                state: "dry-run".into(),
            });
            continue;
        }
        let primary = qroot.join(&entry_name);
        let moved_to = fs::create_dir_all(&qroot)
            .and_then(|_| fs::rename(&item.path, &primary))
            .map(|_| primary)
            .or_else(|_| {
                // Different volume: quarantine next to the item instead.
                let parent = item
                    .path
                    .parent()
                    .ok_or_else(|| io::Error::other("no parent"))?;
                let side = parent
                    .join(".agentdisk-quarantine")
                    .join(format!("{}-{entry_name}", plan.id));
                fs::create_dir_all(side.parent().unwrap())?;
                fs::rename(&item.path, &side).map(|_| side)
            });
        match moved_to {
            Ok(q) => {
                let e = JournalEntry {
                    original: item.path.clone(),
                    quarantine: q,
                    kind: item.kind,
                    repo: item.repo.clone(),
                    bytes: item.bytes,
                    moved_at: size::now_secs(),
                    state: "quarantined".into(),
                };
                journal.push(e.clone());
                // Write after every move so a crash never loses track.
                if let Err(err) = write_journal(&plan.id, &journal) {
                    out.skipped.push((
                        item.path.clone(),
                        format!("moved, but journal write failed: {err}"),
                    ));
                }
                out.moved.push(e);
            }
            Err(e) => out.skipped.push((
                item.path.clone(),
                format!("could not move (in use or no permission): {e}"),
            )),
        }
    }
    out
}

/// Move quarantined items of a plan back. Returns (restored, problems).
pub fn restore(plan_id: &str) -> (Vec<JournalEntry>, Vec<String>) {
    let mut journal = read_journal(plan_id);
    let mut done = Vec::new();
    let mut problems = Vec::new();
    if journal.is_empty() {
        problems.push(format!("nothing in quarantine for plan '{plan_id}'"));
        return (done, problems);
    }
    for e in journal.iter_mut().filter(|e| e.state == "quarantined") {
        if e.original.exists() {
            problems.push(format!(
                "{} exists again; left the copy in {}",
                display(&e.original),
                display(&e.quarantine)
            ));
            continue;
        }
        if let Some(p) = e.original.parent() {
            let _ = fs::create_dir_all(p);
        }
        match fs::rename(&e.quarantine, &e.original) {
            Ok(()) => {
                e.state = "restored".into();
                done.push(e.clone());
            }
            Err(err) => problems.push(format!("{}: {err}", display(&e.original))),
        }
    }
    if let Err(e) = write_journal(plan_id, &journal) {
        problems.push(format!("journal write failed: {e}"));
    }
    (done, problems)
}

/// Quarantined entries older than `min_days` (all plans, or one).
pub fn purge_candidates(plan: Option<&str>, min_days: u64) -> Vec<(String, JournalEntry)> {
    let now = size::now_secs();
    let ids = match plan {
        Some(p) => vec![p.to_string()],
        None => quarantined_plans(),
    };
    let mut out = Vec::new();
    for id in ids {
        for e in read_journal(&id) {
            if e.state == "quarantined" && now.saturating_sub(e.moved_at) >= min_days * 86_400 {
                out.push((id.clone(), e));
            }
        }
    }
    out
}

/// Permanently delete quarantined entries. Returns (freed bytes, problems).
pub fn purge(entries: &[(String, JournalEntry)]) -> (u64, Vec<String>) {
    let mut freed = 0;
    let mut problems = Vec::new();
    let mut by_plan: std::collections::BTreeMap<String, Vec<PathBuf>> = Default::default();
    for (id, e) in entries {
        // For worktrees, find git's admin folder (<repo>/.git/worktrees/<name>)
        // before deleting, so only *this* record is removed. Running a global
        // `git worktree prune` could drop records of other quarantined
        // worktrees and break their restore.
        let admin = (e.kind == ItemKind::Worktree)
            .then(|| worktree_admin_dir(&e.quarantine))
            .flatten();
        sys::make_writable(&e.quarantine);
        match fs::remove_dir_all(&e.quarantine) {
            Ok(()) => {
                freed += e.bytes;
                by_plan
                    .entry(id.clone())
                    .or_default()
                    .push(e.quarantine.clone());
                if let Some(admin) = admin {
                    let _ = fs::remove_dir_all(admin); // branch and commits stay
                }
                if let Some(side) = e
                    .quarantine
                    .parent()
                    .filter(|p| p.ends_with(".agentdisk-quarantine"))
                {
                    let _ = fs::remove_dir(side); // only if empty
                }
            }
            Err(err) => problems.push(format!("{}: {err}", display(&e.quarantine))),
        }
    }
    for (id, done) in by_plan {
        let mut journal = read_journal(&id);
        for j in journal.iter_mut() {
            if done.contains(&j.quarantine) {
                j.state = "purged".into();
            }
        }
        if let Err(e) = write_journal(&id, &journal) {
            problems.push(format!("journal write failed: {e}"));
        }
    }
    (freed, problems)
}

/// `<repo>/.git/worktrees/<name>` for a (moved) linked worktree folder.
fn worktree_admin_dir(wt: &Path) -> Option<PathBuf> {
    let text = fs::read_to_string(wt.join(".git")).ok()?;
    let gitdir = PathBuf::from(text.trim().strip_prefix("gitdir:")?.trim());
    let gitdir = if gitdir.is_absolute() {
        gitdir
    } else {
        wt.join(gitdir)
    };
    // Only ever remove a folder that really is a worktree admin dir.
    let parent_is_worktrees = gitdir
        .parent()
        .and_then(|p| p.file_name())
        .is_some_and(|n| n == "worktrees");
    (parent_is_worktrees && gitdir.join("HEAD").is_file() && gitdir.join("commondir").is_file())
        .then_some(gitdir)
}

/// Overview of everything in quarantine.
pub fn render_quarantine() -> String {
    let ids = quarantined_plans();
    let now = size::now_secs();
    let mut rows = Vec::new();
    for id in &ids {
        for e in read_journal(id) {
            rows.push(vec![
                id.clone(),
                e.state.clone(),
                human(e.bytes),
                util::age(Some(now.saturating_sub(e.moved_at) / 86_400)),
                util::short_path(&display(&e.original), 60),
            ]);
        }
    }
    if rows.is_empty() {
        return "Quarantine is empty.\n".into();
    }
    let mut out = String::from("QUARANTINE\n");
    out.push_str(&table(
        &["Plan", "State", "Size", "Moved", "Original path"],
        &rows,
        &[2, 3],
    ));
    out.push_str("\nRestore: agentdisk restore <plan>   Delete for good: agentdisk purge\n");
    out
}

/// Convenience used by the CLI and MCP server.
pub fn plan_now(opts: &PlanOptions) -> (Plan, Report) {
    let report = scan::run(&opts.scan);
    (make_plan(&report, opts), report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignored_entry_rules() {
        assert!(ignored_is_regenerable("node_modules/"));
        assert!(ignored_is_regenerable("packages/web/node_modules/"));
        assert!(ignored_is_regenerable("target/"));
        assert!(ignored_is_regenerable("debug.log"));
        assert!(!ignored_is_regenerable(".env"));
        assert!(!ignored_is_regenerable("secrets/key.pem"));
        assert!(!ignored_is_regenerable(".claude/settings.local.json"));
    }

    #[test]
    fn worktree_checks() {
        let base = WorktreeItem {
            path: PathBuf::from("/nonexistent/wt"),
            repo: Some(PathBuf::from("/nonexistent")),
            agent: "git".into(),
            head: Some("abc".into()),
            branch: Some("feat".into()),
            locked: false,
            prunable: false,
            orphan: false,
            dirty: Some(0),
            lockfile: None,
            totals: Default::default(),
        };
        let now = 100 * 86_400;
        let mut w = base.clone();
        w.locked = true;
        assert!(check_worktree(&w, now, 7).unwrap_err().contains("locked"));
        let mut w = base.clone();
        w.dirty = Some(3);
        assert!(check_worktree(&w, now, 7)
            .unwrap_err()
            .contains("uncommitted"));
        let mut w = base.clone();
        w.totals.newest_mtime = now - 86_400;
        assert!(check_worktree(&w, now, 7).unwrap_err().contains("idle"));
        let mut w = base;
        w.orphan = true;
        assert!(check_worktree(&w, now, 7).is_err());
    }

    #[test]
    fn plan_code_is_last_six() {
        let p = Plan {
            id: "20261018-a1b2c3".into(),
            created: 0,
            items: vec![],
            skipped: vec![],
            hints: vec![],
        };
        assert_eq!(p.code(), "a1b2c3");
    }
}
