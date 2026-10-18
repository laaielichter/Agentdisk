//! `agentdisk scan`: what every agent left on disk. Read-only.

use crate::adapters::{self, Adapter, Class, Kind};
use crate::git;
use crate::json::Value;
use crate::paths::{self, canon, display, slashed};
use crate::projects::{self, EcoRule};
use crate::size::{self, human, Totals};
use crate::util::{self, table};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct ScanOptions {
    pub projects: Vec<PathBuf>,
    pub adapter_dirs: Vec<PathBuf>,
    pub threads: usize,
    pub max_depth: usize,
    /// Measure agent data dirs from adapters.
    pub agents: bool,
}

impl Default for ScanOptions {
    fn default() -> Self {
        ScanOptions {
            projects: Vec::new(),
            adapter_dirs: Vec::new(),
            threads: size::default_threads(),
            max_depth: 6,
            agents: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AgentItem {
    pub adapter: String,
    pub agent: String,
    pub what: String,
    pub path: PathBuf,
    pub class: Class,
    pub documented: bool,
    pub note: Option<String>,
    pub totals: Totals,
    /// Inside another listed agent path (excluded from sums).
    pub nested: bool,
}

#[derive(Debug, Clone)]
pub struct WorktreeItem {
    pub path: PathBuf,
    pub repo: Option<PathBuf>,
    pub agent: String,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub locked: bool,
    pub prunable: bool,
    pub orphan: bool,
    /// Files changed or untracked (`None` = unknown).
    pub dirty: Option<usize>,
    pub lockfile: Option<(String, u64)>,
    pub totals: Totals,
}

#[derive(Debug, Clone)]
pub struct EcoItem {
    pub path: PathBuf,
    pub rule: &'static EcoRule,
    pub project: PathBuf,
    pub worktree: Option<PathBuf>,
    pub totals: Totals,
}

#[derive(Debug, Default)]
pub struct Report {
    pub generated: u64,
    pub agents: Vec<AgentItem>,
    pub worktrees: Vec<WorktreeItem>,
    pub eco: Vec<EcoItem>,
    pub adapters: Vec<Adapter>,
    pub warnings: Vec<String>,
    pub disk: Option<(u64, u64)>,
    pub projects: Vec<PathBuf>,
}

fn key(p: &Path) -> String {
    let s = slashed(&canon(p));
    if cfg!(windows) || cfg!(target_os = "macos") {
        s.to_lowercase()
    } else {
        s
    }
}

pub fn load_adapters(extra: &[PathBuf]) -> (Vec<Adapter>, Vec<String>) {
    let mut dirs: Vec<PathBuf> = vec![paths::config_dir().join("adapters")];
    dirs.extend(extra.iter().cloned());
    let refs: Vec<&Path> = dirs.iter().map(PathBuf::as_path).collect();
    adapters::load(&refs)
}

/// Which agent made this worktree, judged by its path.
pub fn agent_for_path(adapters: &[Adapter], p: &Path) -> String {
    let s = slashed(p);
    for a in adapters {
        for e in &a.paths {
            if e.kind == Kind::Worktree {
                if let Some(m) = &e.marker {
                    if s.contains(m.as_str()) {
                        return a.id.clone();
                    }
                }
            }
        }
    }
    "git".to_string()
}

fn lockfile(dir: &Path) -> Option<(String, u64)> {
    const LOCKS: &[&str] = &[
        "pnpm-lock.yaml",
        "package-lock.json",
        "yarn.lock",
        "bun.lock",
        "bun.lockb",
        "uv.lock",
        "poetry.lock",
        "Pipfile.lock",
        "requirements.txt",
        "Cargo.lock",
    ];
    LOCKS
        .iter()
        .find_map(|n| util::fnv_file(&dir.join(n)).map(|h| (n.to_string(), h)))
}

pub fn run(opts: &ScanOptions) -> Report {
    let (adapters, mut warnings) = load_adapters(&opts.adapter_dirs);
    let mut report = Report {
        generated: size::now_secs(),
        disk: crate::sys::disk_space(&paths::home()),
        projects: opts.projects.clone(),
        ..Default::default()
    };

    // 1. Agent data folders.
    let mut adapter_worktree_dirs: Vec<PathBuf> = Vec::new();
    for a in &adapters {
        for e in a.paths.iter().filter(|e| e.applies_here()) {
            let found = paths::expand(&e.template);
            match e.kind {
                Kind::Worktree => adapter_worktree_dirs.extend(found),
                Kind::Dir if opts.agents => {
                    for p in found {
                        match size::measure(&p, opts.threads) {
                            Ok(t) => report.agents.push(AgentItem {
                                adapter: a.id.clone(),
                                agent: a.name.clone(),
                                what: e.id.clone(),
                                path: p,
                                class: e.class,
                                documented: e.documented,
                                note: e.note.clone(),
                                totals: t,
                                nested: false,
                            }),
                            Err(err) => warnings.push(format!("{}: {err}", display(&p))),
                        }
                    }
                }
                Kind::Dir => {}
            }
        }
    }
    let agent_keys: Vec<(usize, String)> = report
        .agents
        .iter()
        .enumerate()
        .map(|(i, a)| (i, key(&a.path)))
        .collect();
    for (i, k) in &agent_keys {
        let nested = agent_keys
            .iter()
            .any(|(j, other)| j != i && k.starts_with(&format!("{other}/")));
        report.agents[*i].nested = nested;
    }

    // 2. Projects and agent worktree folders.
    let mut roots: Vec<PathBuf> = opts
        .projects
        .iter()
        .filter(|p| p.exists())
        .cloned()
        .collect();
    for p in &opts.projects {
        if !p.exists() {
            warnings.push(format!("project folder not found: {}", display(p)));
        }
    }
    roots.extend(adapter_worktree_dirs.iter().cloned());
    let scan = projects::walk(&roots, opts.max_depth);

    // 3. Worktrees, via git.
    let have_git = git::available();
    if !have_git && (!scan.repos.is_empty() || !scan.linked.is_empty()) {
        warnings.push("git not found: worktree details are limited".into());
    }
    let mut repo_roots: Vec<PathBuf> = scan.repos.clone();
    let mut orphans: Vec<PathBuf> = Vec::new();
    for l in scan.linked.iter().chain(adapter_worktree_dirs.iter()) {
        match have_git.then(|| git::common_dir(l)).flatten() {
            Some(c) => {
                let root = if c.file_name().is_some_and(|n| n == ".git") {
                    c.parent().map(Path::to_path_buf).unwrap_or(c)
                } else {
                    c
                };
                repo_roots.push(root);
            }
            None => orphans.push(l.clone()),
        }
    }
    let mut seen_repo = HashSet::new();
    let mut seen_wt: HashSet<String> = HashSet::new();
    for repo in repo_roots {
        if !seen_repo.insert(key(&repo)) {
            continue;
        }
        for w in git::worktrees(&repo) {
            if w.is_main || w.bare || !seen_wt.insert(key(&w.path)) {
                continue;
            }
            let exists = w.path.exists();
            let totals = if exists {
                size::measure(&w.path, opts.threads).unwrap_or_default()
            } else {
                Totals::default()
            };
            report.worktrees.push(WorktreeItem {
                agent: agent_for_path(&adapters, &w.path),
                dirty: if exists {
                    git::dirty_files(&w.path).map(|d| d.len())
                } else {
                    None
                },
                lockfile: if exists { lockfile(&w.path) } else { None },
                repo: Some(canon(&repo)),
                path: w.path,
                head: w.head,
                branch: w.branch,
                locked: w.locked,
                prunable: w.prunable || !exists,
                orphan: false,
                totals,
            });
        }
    }
    for o in orphans {
        if !seen_wt.insert(key(&o)) {
            continue;
        }
        report.worktrees.push(WorktreeItem {
            agent: agent_for_path(&adapters, &o),
            totals: size::measure(&o, opts.threads).unwrap_or_default(),
            lockfile: lockfile(&o),
            path: o,
            repo: None,
            head: None,
            branch: None,
            locked: false,
            prunable: false,
            orphan: true,
            dirty: None,
        });
    }
    report
        .worktrees
        .sort_by_key(|x| std::cmp::Reverse(x.totals.allocated_bytes));

    // 4. Dependency and build folders.
    let wt_keys: Vec<(String, PathBuf)> = report
        .worktrees
        .iter()
        .map(|w| (key(&w.path), w.path.clone()))
        .collect();
    let mut seen_eco = HashSet::new();
    for e in scan.eco {
        let k = key(&e.path);
        if !seen_eco.insert(k.clone()) {
            continue;
        }
        let worktree = wt_keys
            .iter()
            .find(|(wk, _)| k.starts_with(&format!("{wk}/")))
            .map(|(_, p)| p.clone());
        let totals = size::measure(&e.path, opts.threads).unwrap_or_default();
        report.eco.push(EcoItem {
            path: e.path,
            rule: e.rule,
            project: e.project,
            worktree,
            totals,
        });
    }
    report
        .eco
        .sort_by_key(|x| std::cmp::Reverse(x.totals.allocated_bytes));

    report.adapters = adapters;
    report.warnings = warnings;
    report
}

pub struct Summary {
    pub agent_user: u64,
    pub agent_regen: u64,
    pub agent_other: u64,
    pub worktrees: u64,
    pub worktree_reclaim: u64,
    pub eco_outside: u64,
    pub eco_outside_reclaim: u64,
}

pub fn summary(r: &Report) -> Summary {
    let mut s = Summary {
        agent_user: 0,
        agent_regen: 0,
        agent_other: 0,
        worktrees: 0,
        worktree_reclaim: 0,
        eco_outside: 0,
        eco_outside_reclaim: 0,
    };
    for a in r.agents.iter().filter(|a| !a.nested) {
        match a.class {
            Class::UserData => s.agent_user += a.totals.allocated_bytes,
            Class::Regenerable => s.agent_regen += a.totals.allocated_bytes,
            _ => s.agent_other += a.totals.allocated_bytes,
        }
    }
    for w in &r.worktrees {
        s.worktrees += w.totals.allocated_bytes;
        s.worktree_reclaim += w.totals.reclaimable_bytes;
    }
    for e in r.eco.iter().filter(|e| e.worktree.is_none()) {
        s.eco_outside += e.totals.allocated_bytes;
        s.eco_outside_reclaim += e.totals.reclaimable_bytes;
    }
    s
}

fn wt_status(w: &WorktreeItem) -> String {
    let mut st = Vec::new();
    if w.orphan {
        st.push("orphan".to_string());
    }
    if w.prunable {
        st.push("missing".to_string());
    }
    if w.locked {
        st.push("locked".to_string());
    }
    match w.dirty {
        Some(0) => st.push("clean".to_string()),
        Some(n) => st.push(format!("{n} changed")),
        None => {}
    }
    if w.head.is_some() && w.branch.is_none() && !w.orphan {
        st.push("detached".to_string());
    }
    st.join(", ")
}

/// Human-readable report.
pub fn render(r: &Report) -> String {
    let now = r.generated;
    let mut out = format!("agentdisk scan — {}\n", util::datetime(now));
    out.push_str("Read-only. Nothing was changed.\n\n");

    out.push_str("AGENT DATA\n");
    if r.agents.is_empty() {
        out.push_str("  No agent data found.\n");
    } else {
        let mut rows: Vec<&AgentItem> = r.agents.iter().collect();
        rows.sort_by_key(|x| std::cmp::Reverse(x.totals.allocated_bytes));
        let rows: Vec<Vec<String>> = rows
            .iter()
            .map(|a| {
                vec![
                    a.agent.clone(),
                    a.what.clone() + if a.documented { "" } else { " *" },
                    a.class.to_string(),
                    human(a.totals.allocated_bytes),
                    util::age(a.totals.idle_days(now)),
                    util::short_path(&display(&a.path), 60),
                ]
            })
            .collect();
        out.push_str(&table(
            &["Agent", "What", "Class", "On disk", "Idle", "Path"],
            &rows,
            &[3, 4],
        ));
        out.push_str("  * = undocumented location, report only.\n");
    }

    out.push_str(&format!("\nWORKTREES ({})\n", r.worktrees.len()));
    if r.worktrees.is_empty() {
        if r.projects.is_empty() {
            out.push_str(
                "  None in agent folders. Add --projects <folder> to scan your repositories too.\n",
            );
        } else {
            out.push_str("  No extra worktrees found.\n");
        }
    } else {
        let rows: Vec<Vec<String>> = r
            .worktrees
            .iter()
            .map(|w| {
                vec![
                    w.agent.clone(),
                    w.repo
                        .as_ref()
                        .and_then(|p| p.file_name())
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "?".into()),
                    w.branch.clone().unwrap_or_else(|| "-".into()),
                    util::age(w.totals.idle_days(now)),
                    human(w.totals.allocated_bytes),
                    human(w.totals.reclaimable_bytes),
                    wt_status(w),
                    util::short_path(&display(&w.path), 50),
                ]
            })
            .collect();
        out.push_str(&table(
            &[
                "Agent",
                "Repo",
                "Branch",
                "Idle",
                "On disk",
                "Reclaimable",
                "Status",
                "Path",
            ],
            &rows,
            &[3, 4, 5],
        ));
    }

    out.push_str(&format!("\nDEPENDENCY & BUILD FOLDERS ({})\n", r.eco.len()));
    if r.eco.is_empty() {
        out.push_str("  None found.\n");
    } else {
        let rows: Vec<Vec<String>> = r
            .eco
            .iter()
            .take(40)
            .map(|e| {
                vec![
                    e.rule.dir.to_string(),
                    e.rule.ecosystem.to_string(),
                    if e.worktree.is_some() {
                        "worktree"
                    } else {
                        "project"
                    }
                    .to_string(),
                    util::age(e.totals.idle_days(now)),
                    human(e.totals.allocated_bytes),
                    human(e.totals.reclaimable_bytes),
                    util::short_path(&display(&e.path), 60),
                ]
            })
            .collect();
        out.push_str(&table(
            &[
                "Folder",
                "Type",
                "In",
                "Idle",
                "On disk",
                "Reclaimable",
                "Path",
            ],
            &rows,
            &[3, 4, 5],
        ));
        if r.eco.len() > 40 {
            out.push_str(&format!(
                "  … and {} more (use --json for all)\n",
                r.eco.len() - 40
            ));
        }
    }

    let s = summary(r);
    out.push_str("\nSUMMARY\n");
    out.push_str(&format!(
        "  Agent data:            {} user data, {} regenerable, {} other\n",
        human(s.agent_user),
        human(s.agent_regen),
        human(s.agent_other)
    ));
    out.push_str(&format!(
        "  Worktrees:             {} on disk, {} reclaimable\n",
        human(s.worktrees),
        human(s.worktree_reclaim)
    ));
    out.push_str(&format!(
        "  Build/deps (projects): {} on disk, {} reclaimable\n",
        human(s.eco_outside),
        human(s.eco_outside_reclaim)
    ));
    if let Some((free, total)) = r.disk {
        out.push_str(&format!(
            "  Free on home drive:    {} of {}\n",
            human(free),
            human(total)
        ));
    }

    let present: HashSet<&str> = r
        .agents
        .iter()
        .map(|a| a.adapter.as_str())
        .chain(r.worktrees.iter().map(|w| w.agent.as_str()))
        .collect();
    let policies: Vec<String> = r
        .adapters
        .iter()
        .filter(|a| present.contains(a.id.as_str()))
        .filter_map(|a| a.policy.as_ref().map(|p| format!("  {}: {p}", a.name)))
        .collect();
    if !policies.is_empty() {
        out.push_str("\nWHAT YOUR AGENTS CLEAN UP THEMSELVES\n");
        for p in policies {
            out.push_str(&p);
            out.push('\n');
        }
    }
    if !r.warnings.is_empty() {
        out.push_str("\nWARNINGS\n");
        for w in &r.warnings {
            out.push_str(&format!("  - {w}\n"));
        }
    }
    out.push_str(
        "\nNext: `agentdisk advise` for savings tips, `agentdisk plan` for a safe cleanup plan.\n",
    );
    out
}

fn totals_json(t: &Totals) -> Value {
    Value::obj(vec![
        ("files", t.files.into()),
        ("dirs", t.dirs.into()),
        ("logical_bytes", t.logical_bytes.into()),
        ("allocated_bytes", t.allocated_bytes.into()),
        ("reclaimable_bytes", t.reclaimable_bytes.into()),
        ("hardlinked_files", t.hardlinked_files.into()),
        ("links_skipped", t.links_skipped.into()),
        ("errors", t.errors.into()),
        ("newest_mtime", t.newest_mtime.into()),
    ])
}

pub const SCHEMA_VERSION: u64 = 1;

pub fn to_json(r: &Report) -> Value {
    let s = summary(r);
    let agents = r
        .agents
        .iter()
        .map(|a| {
            Value::obj(vec![
                ("adapter", a.adapter.as_str().into()),
                ("agent", a.agent.as_str().into()),
                ("what", a.what.as_str().into()),
                ("path", display(&a.path).into()),
                ("class", a.class.as_str().into()),
                ("documented", a.documented.into()),
                ("nested", a.nested.into()),
                ("note", a.note.clone().into()),
                ("totals", totals_json(&a.totals)),
            ])
        })
        .collect::<Vec<_>>();
    let worktrees = r
        .worktrees
        .iter()
        .map(|w| {
            Value::obj(vec![
                ("path", display(&w.path).into()),
                ("repo", w.repo.as_ref().map(|p| display(p)).into()),
                ("agent", w.agent.as_str().into()),
                ("head", w.head.clone().into()),
                ("branch", w.branch.clone().into()),
                ("locked", w.locked.into()),
                ("prunable", w.prunable.into()),
                ("orphan", w.orphan.into()),
                ("dirty_files", w.dirty.map(|d| d as u64).into()),
                (
                    "lockfile",
                    w.lockfile.as_ref().map(|(n, _)| n.as_str()).into(),
                ),
                ("totals", totals_json(&w.totals)),
            ])
        })
        .collect::<Vec<_>>();
    let eco = r
        .eco
        .iter()
        .map(|e| {
            Value::obj(vec![
                ("path", display(&e.path).into()),
                ("folder", e.rule.dir.into()),
                ("ecosystem", e.rule.ecosystem.into()),
                ("project", display(&e.project).into()),
                ("worktree", e.worktree.as_ref().map(|p| display(p)).into()),
                ("rebuild", e.rule.rebuild.into()),
                ("totals", totals_json(&e.totals)),
            ])
        })
        .collect::<Vec<_>>();
    let mut policies = BTreeMap::new();
    for a in &r.adapters {
        if let Some(p) = &a.policy {
            policies.insert(a.id.clone(), p.clone());
        }
    }
    Value::obj(vec![
        ("schema_version", SCHEMA_VERSION.into()),
        ("tool", "agentdisk".into()),
        ("version", crate::VERSION.into()),
        ("generated", r.generated.into()),
        ("agents", agents.into()),
        ("worktrees", worktrees.into()),
        ("build_folders", eco.into()),
        (
            "summary",
            Value::obj(vec![
                ("agent_user_data_bytes", s.agent_user.into()),
                ("agent_regenerable_bytes", s.agent_regen.into()),
                ("agent_other_bytes", s.agent_other.into()),
                ("worktree_bytes", s.worktrees.into()),
                ("worktree_reclaimable_bytes", s.worktree_reclaim.into()),
                ("project_build_bytes", s.eco_outside.into()),
                (
                    "project_build_reclaimable_bytes",
                    s.eco_outside_reclaim.into(),
                ),
                ("free_bytes", r.disk.map(|d| d.0).into()),
                ("total_bytes", r.disk.map(|d| d.1).into()),
            ]),
        ),
        (
            "policies",
            Value::Obj(
                policies
                    .into_iter()
                    .map(|(k, v)| (k, Value::Str(v)))
                    .collect(),
            ),
        ),
        (
            "warnings",
            r.warnings
                .iter()
                .map(|w| Value::from(w.as_str()))
                .collect::<Vec<_>>()
                .into(),
        ),
    ])
}
