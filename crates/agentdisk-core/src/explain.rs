//! `agentdisk explain <path>`: what is this folder, who made it, and may it
//! be cleaned? Read-only. Uses the same rules as `plan`.

use crate::adapters::{Class, Kind};
use crate::cleanup;
use crate::config;
use crate::git;
use crate::json::Value;
use crate::paths::{self, canon, display};
use crate::projects;
use crate::scan::{self, key, WorktreeItem};
use crate::size::{self, human, Totals};
use crate::sys;
use crate::util;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// `plan` would include it.
    Cleanable(String),
    /// `plan` would skip it.
    Keep(String),
    /// Not something agentdisk manages.
    Unknown,
}

#[derive(Debug, Clone)]
pub struct Explanation {
    pub path: PathBuf,
    pub totals: Totals,
    /// What the folder is, most specific first.
    pub facts: Vec<String>,
    pub owner: Option<String>,
    pub class: Option<Class>,
    pub verdict: Verdict,
}

fn inside(child: &str, parent: &str) -> bool {
    child == parent || child.starts_with(&format!("{parent}/"))
}

/// Nearest ancestor (or self) with a `.git` *file* = linked worktree root.
fn worktree_root(p: &Path) -> Option<PathBuf> {
    p.ancestors()
        .find(|a| a.join(".git").is_file())
        .map(Path::to_path_buf)
}

pub fn explain(path: &Path) -> Result<Explanation, String> {
    let meta = std::fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", display(path)))?;
    if meta.file_type().is_symlink() {
        return Ok(Explanation {
            path: path.to_path_buf(),
            totals: Totals::default(),
            facts: vec![
                "This is a link (symlink or junction). agentdisk never follows or removes links."
                    .into(),
            ],
            owner: None,
            class: None,
            verdict: Verdict::Keep("links are never touched".into()),
        });
    }
    let p = canon(path);
    let k = key(&p);
    let cfg = config::load();
    let now = size::now_secs();
    let totals = size::measure(&p, size::default_threads()).map_err(|e| e.to_string())?;
    let procs = sys::process_names();
    let mut facts = Vec::new();
    let mut owner = None;
    let mut class = None;
    let mut verdict = Verdict::Unknown;

    // 1. Agent and tool data (most specific match wins).
    let (adapters, _) = scan::load_adapters(&cfg.adapters);
    let mut best: Option<(
        usize,
        &crate::adapters::Adapter,
        &crate::adapters::PathEntry,
        PathBuf,
    )> = None;
    for a in &adapters {
        for e in a
            .paths
            .iter()
            .filter(|e| e.applies_here() && e.kind == Kind::Dir)
        {
            for root in paths::expand(&e.template) {
                let rk = key(&root);
                if inside(&k, &rk) && best.as_ref().is_none_or(|b| rk.len() > b.0) {
                    best = Some((rk.len(), a, e, root));
                }
            }
        }
    }
    if let Some((_, a, e, root)) = &best {
        let exact = key(root) == k;
        facts.push(format!(
            "{} {} of {}{}{}",
            if exact { "This is the" } else { "Inside the" },
            e.id,
            a.name,
            if e.documented {
                ""
            } else {
                " (undocumented location)"
            },
            e.note
                .as_ref()
                .map(|n| format!(". {n}"))
                .unwrap_or_default()
        ));
        if let Some(pol) = &a.policy {
            facts.push(format!("{} cleans up by itself: {pol}", a.name));
        }
        if let Some(c) = &a.cleanup {
            facts.push(format!("Vendor cleanup command: {c}"));
        }
        owner = Some(a.id.clone());
        class = Some(e.class);
        verdict = match e.class {
            Class::UserData => {
                Verdict::Keep("user data (history or work); agentdisk never plans it".into())
            }
            Class::Protected => {
                Verdict::Keep("settings, memories or extensions; never touched".into())
            }
            Class::Model => {
                Verdict::Keep("large download (models); remove it with the tool itself".into())
            }
            Class::Download => Verdict::Keep(
                "re-downloadable tool files; remove old versions in the tool itself".into(),
            ),
            Class::Regenerable if !exact => Verdict::Keep(format!(
                "part of a regenerable folder; clean {} as a whole",
                display(root)
            )),
            Class::Regenerable => {
                let idle = totals.idle_days(now).unwrap_or(0);
                if a.processes
                    .iter()
                    .any(|n| cleanup::process_matches(&procs, n))
                {
                    Verdict::Keep(format!("{} is running", a.name))
                } else if idle < cfg.worktree_days {
                    Verdict::Keep(format!(
                        "changed {} — needs {}+ days idle",
                        util::age(Some(idle)),
                        cfg.worktree_days
                    ))
                } else {
                    Verdict::Cleanable(format!("regenerable, idle {}", util::age(Some(idle))))
                }
            }
        };
    }

    // 2. Git: main repository or linked worktree.
    if p.join(".git").is_dir() {
        facts.push("This is a main git repository. agentdisk never removes repositories.".into());
        verdict = Verdict::Keep("main repository".into());
    } else if let Some(wt) = worktree_root(&p) {
        let agent = scan::agent_for_path(&adapters, &wt);
        let repo = git::common_dir(&wt).map(|c| {
            if c.file_name().is_some_and(|n| n == ".git") {
                c.parent().map(Path::to_path_buf).unwrap_or(c)
            } else {
                c
            }
        });
        if key(&wt) == k {
            facts.push(format!("This is a git worktree made by {agent}."));
            owner.get_or_insert(agent.clone());
            let listed = repo
                .as_ref()
                .and_then(|r| git::worktrees(r).into_iter().find(|w| key(&w.path) == k));
            verdict = match (repo, listed) {
                (Some(r), Some(w)) => {
                    facts.push(format!("Repository: {}", display(&r)));
                    if let Some(b) = &w.branch {
                        facts.push(format!("Branch: {b} (always kept)"));
                    }
                    let item = WorktreeItem {
                        dirty: git::dirty_files(&p).map(|d| d.len()),
                        path: p.clone(),
                        repo: Some(r),
                        agent,
                        head: w.head,
                        branch: w.branch,
                        locked: w.locked,
                        prunable: w.prunable,
                        orphan: false,
                        lockfile: None,
                        totals,
                    };
                    match cleanup::check_worktree(&item, now, cfg.worktree_days) {
                        Ok(r) => Verdict::Cleanable(r),
                        Err(r) => Verdict::Keep(r),
                    }
                }
                _ => Verdict::Keep("its repository was not found; check it by hand".into()),
            };
        } else {
            facts.push(format!(
                "Inside the git worktree {} (made by {agent}).",
                display(&wt)
            ));
        }
    }

    // 3. Dependency / build folders.
    if let Some(rule) = projects::rule_for(&p) {
        facts.push(format!(
            "This is a {} folder ({}). Rebuild: {}.",
            rule.dir, rule.ecosystem, rule.rebuild
        ));
        owner.get_or_insert(rule.ecosystem.to_string());
        class.get_or_insert(Class::Regenerable);
        verdict = match cleanup::check_build(
            &p,
            totals.idle_days(now),
            totals.reclaimable_bytes,
            cfg.build_days,
            &procs,
        ) {
            Ok(r) => Verdict::Cleanable(r),
            Err(r) => Verdict::Keep(r),
        };
    } else if let Some(anc) = p
        .ancestors()
        .skip(1)
        .find(|a| projects::rule_for(a).is_some())
    {
        facts.push(format!(
            "Inside the build folder {}. Clean that folder as a whole, not parts of it.",
            display(anc)
        ));
        if verdict == Verdict::Unknown {
            verdict = Verdict::Keep("part of a build folder".into());
        }
    }

    if facts.is_empty() {
        facts.push("agentdisk does not know this folder. It is never planned for cleanup.".into());
    }
    if totals.hardlinked_files > 0 && totals.reclaimable_bytes < totals.allocated_bytes {
        facts.push(format!(
            "{} hardlinked file(s) are shared with other folders; deleting frees only {}.",
            totals.hardlinked_files,
            human(totals.reclaimable_bytes)
        ));
    }
    Ok(Explanation {
        path: p,
        totals,
        facts,
        owner,
        class,
        verdict,
    })
}

pub fn render(e: &Explanation) -> String {
    let mut out = format!("{}\n", display(&e.path));
    out.push_str(&format!(
        "  on disk {}   freed if deleted {}   idle {}\n\n",
        human(e.totals.allocated_bytes),
        human(e.totals.reclaimable_bytes),
        util::age(e.totals.idle_days(size::now_secs()))
    ));
    for f in &e.facts {
        out.push_str(&format!("  - {f}\n"));
    }
    out.push('\n');
    out.push_str(&match &e.verdict {
        Verdict::Cleanable(r) => format!("  Verdict: can be cleaned ({r}).\n  Use `agentdisk plan` to include it; nothing is removed by explain.\n"),
        Verdict::Keep(r) => format!("  Verdict: keep ({r}).\n"),
        Verdict::Unknown => "  Verdict: not managed by agentdisk.\n".into(),
    });
    out
}

pub fn to_json(e: &Explanation) -> Value {
    let (v, reason) = match &e.verdict {
        Verdict::Cleanable(r) => ("cleanable", Some(r.as_str())),
        Verdict::Keep(r) => ("keep", Some(r.as_str())),
        Verdict::Unknown => ("unknown", None),
    };
    Value::obj(vec![
        ("path", display(&e.path).into()),
        ("owner", e.owner.clone().into()),
        ("class", e.class.map(|c| c.as_str()).into()),
        ("verdict", v.into()),
        ("reason", reason.into()),
        (
            "facts",
            e.facts
                .iter()
                .map(|f| Value::from(f.as_str()))
                .collect::<Vec<_>>()
                .into(),
        ),
        ("allocated_bytes", e.totals.allocated_bytes.into()),
        ("reclaimable_bytes", e.totals.reclaimable_bytes.into()),
        ("newest_mtime", e.totals.newest_mtime.into()),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::temp_dir;
    use std::fs;

    #[test]
    fn explains_build_folder_and_unknown() {
        let root = temp_dir("explain");
        fs::create_dir_all(root.join("web/node_modules/a")).unwrap();
        fs::write(root.join("web/package.json"), "{}").unwrap();
        fs::write(root.join("web/node_modules/a/x.js"), vec![1u8; 5000]).unwrap();
        let e = explain(&root.join("web/node_modules")).unwrap();
        assert!(e.facts.iter().any(|f| f.contains("node_modules")));
        assert!(
            matches!(e.verdict, Verdict::Keep(_)),
            "fresh folder is kept: {:?}",
            e.verdict
        );
        let inner = explain(&root.join("web/node_modules/a")).unwrap();
        assert!(inner
            .facts
            .iter()
            .any(|f| f.contains("Inside the build folder")));
        let unknown = explain(&root.join("web")).unwrap();
        assert_eq!(unknown.verdict, Verdict::Unknown);
        assert!(explain(&root.join("missing")).is_err());
        fs::remove_dir_all(&root).unwrap();
    }
}
