//! Read-only git queries. `GIT_OPTIONAL_LOCKS=0` stops `git status` from
//! rewriting the index, so scanning never changes a repository.

use std::path::{Path, PathBuf};
use std::process::Command;

fn cmd(dir: &Path, args: &[&str]) -> Command {
    let mut c = Command::new("git");
    c.arg("-C")
        .arg(crate::paths::plain(dir))
        .args(["-c", "core.quotepath=off"])
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C");
    c
}

/// stdout of a successful git command.
pub fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = cmd(dir, args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Exit status only (for `check-ignore`).
fn git_ok(dir: &Path, args: &[&str]) -> Option<bool> {
    cmd(dir, args).output().ok().map(|o| o.status.success())
}

pub fn available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Worktree {
    pub path: PathBuf,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub locked: bool,
    pub prunable: bool,
    pub bare: bool,
    pub is_main: bool,
}

/// All worktrees of the repository containing `repo`.
pub fn worktrees(repo: &Path) -> Vec<Worktree> {
    let Some(out) = git(repo, &["worktree", "list", "--porcelain"]) else {
        return Vec::new();
    };
    parse_worktrees(&out)
}

pub fn parse_worktrees(out: &str) -> Vec<Worktree> {
    let mut list = Vec::new();
    let mut cur: Option<Worktree> = None;
    for line in out.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            if let Some(w) = cur.take() {
                list.push(w);
            }
            cur = Some(Worktree {
                path: PathBuf::from(p),
                is_main: list.is_empty(),
                ..Default::default()
            });
        } else if let Some(w) = cur.as_mut() {
            if let Some(h) = line.strip_prefix("HEAD ") {
                w.head = Some(h.to_string());
            } else if let Some(b) = line.strip_prefix("branch ") {
                w.branch = Some(b.trim_start_matches("refs/heads/").to_string());
            } else if line == "locked" || line.starts_with("locked ") {
                w.locked = true;
            } else if line == "prunable" || line.starts_with("prunable ") {
                w.prunable = true;
            } else if line == "bare" {
                w.bare = true;
            }
        }
    }
    if let Some(w) = cur {
        list.push(w);
    }
    list
}

/// Absolute path of the shared `.git` directory for any worktree path.
pub fn common_dir(path: &Path) -> Option<PathBuf> {
    let out = git(
        path,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let p = PathBuf::from(out.trim());
    p.exists().then_some(p)
}

/// Untracked or modified files (ignored files excluded). `None` = git failed.
pub fn dirty_files(wt: &Path) -> Option<Vec<String>> {
    let out = git(wt, &["status", "--porcelain", "--untracked-files=all"])?;
    Some(
        out.lines()
            .map(|l| l.get(3..).unwrap_or(l).to_string())
            .collect(),
    )
}

/// Ignored paths in a worktree (directories collapsed), e.g. `node_modules/`.
pub fn ignored_entries(wt: &Path) -> Option<Vec<String>> {
    let out = git(
        wt,
        &[
            "ls-files",
            "--others",
            "--ignored",
            "--exclude-standard",
            "--directory",
            "--no-empty-directory",
        ],
    )?;
    Some(out.lines().map(str::to_string).collect())
}

/// Is `path` ignored by git? `None` when not inside a repository.
pub fn is_ignored(path: &Path) -> Option<bool> {
    let parent = path.parent()?;
    let name = path.file_name()?.to_string_lossy().into_owned();
    git(parent, &["rev-parse", "--is-inside-work-tree"])?;
    git_ok(parent, &["check-ignore", "-q", "--", &name])
}

/// Does `path` contain files tracked by git?
pub fn has_tracked(path: &Path) -> bool {
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return false;
    };
    let name = name.to_string_lossy().into_owned();
    git(parent, &["ls-files", "--", &name]).is_some_and(|o| !o.trim().is_empty())
}

/// Is commit `sha` reachable from any branch, remote branch or tag?
pub fn reachable(repo: &Path, sha: &str) -> bool {
    git(
        repo,
        &[
            "for-each-ref",
            "--count=1",
            "--contains",
            sha,
            "refs/heads",
            "refs/remotes",
            "refs/tags",
        ],
    )
    .is_some_and(|o| !o.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_porcelain() {
        let out = "worktree /r\nHEAD aaa\nbranch refs/heads/main\n\nworktree /r/.claude/worktrees/x\nHEAD bbb\ndetached\nlocked claude agent running\n\nworktree /gone\nHEAD ccc\nbranch refs/heads/feat\nprunable gitdir file points to non-existent location\n";
        let w = parse_worktrees(out);
        assert_eq!(w.len(), 3);
        assert!(w[0].is_main && w[0].branch.as_deref() == Some("main"));
        assert!(w[1].locked && w[1].branch.is_none() && !w[1].is_main);
        assert!(w[2].prunable);
    }
}
