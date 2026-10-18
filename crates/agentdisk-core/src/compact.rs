//! `agentdisk compact`: transparent compression for folders that are mostly
//! read. On Windows this uses `compact.exe /EXE` (WOF), which decompresses a
//! file as soon as something writes to it, so it only pays off on folders
//! that do not change (SDKs, engine installs, archived projects).
//! Other systems get the right command printed instead.

use crate::advise;
use crate::paths::{self, canon, display, slashed};
use crate::size::{self, human, Totals};
use std::path::{Path, PathBuf};

pub const ALGORITHMS: &[&str] = &["xpress4k", "xpress8k", "xpress16k", "lzx"];

/// Minimum days a folder must be unchanged.
pub const MIN_IDLE_DAYS: u64 = 7;

#[derive(Debug)]
pub struct CompactCheck {
    pub path: PathBuf,
    pub before: Totals,
}

/// Refuse folders where compression is useless or harmful.
pub fn check(path: &Path) -> Result<CompactCheck, String> {
    let meta = std::fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", display(path)))?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err("give a folder (not a file or link)".into());
    }
    let p = canon(path);
    let s = slashed(&p).to_lowercase();
    let home = slashed(&canon(&paths::home())).to_lowercase();
    if s.len() <= 3 || s == home || p.parent().is_none() {
        return Err("refusing to compact a drive root or your whole home folder".into());
    }
    let parts: Vec<&str> = s.split('/').collect();
    for bad in [".git", ".godot", ".import", "node_modules", ".venv"] {
        if parts.contains(&bad) {
            return Err(format!(
                "'{bad}' folders change often or are already handled; skip them"
            ));
        }
    }
    let sys32 = ["/windows/", "/program files/windows", "/system32"];
    if sys32.iter().any(|x| s.contains(x)) {
        return Err(
            "system folders are managed by Windows (use `compact /compactos` instead)".into(),
        );
    }
    for shared in [advise::pnpm_store(), advise::uv_cache()] {
        let sh = slashed(&canon(&shared)).to_lowercase();
        if s.starts_with(&sh) || sh.starts_with(&format!("{s}/")) {
            return Err(format!(
                "{} holds linked package files; compressing them breaks sharing",
                display(&shared)
            ));
        }
    }
    let before = size::measure(&p, size::default_threads()).map_err(|e| e.to_string())?;
    let idle = before.idle_days(size::now_secs()).unwrap_or(0);
    if idle < MIN_IDLE_DAYS {
        return Err(format!(
            "something in this folder changed {idle} day(s) ago; WOF compression is undone by writes, so pick folders unchanged for {MIN_IDLE_DAYS}+ days"
        ));
    }
    Ok(CompactCheck { path: p, before })
}

/// The command that would run (also shown on other systems).
pub fn command(path: &Path, algo: &str, undo: bool) -> String {
    let d = display(path);
    if cfg!(windows) {
        if undo {
            format!("compact /u /s /a /i /exe \"{d}\\*\"")
        } else {
            format!("compact /c /s /a /i /exe:{algo} \"{d}\\*\"")
        }
    } else if cfg!(target_os = "macos") {
        if undo {
            format!("afsctool -d \"{d}\"")
        } else {
            format!("afsctool -c -T LZFSE \"{d}\"   # or: applesauce compress \"{d}\"")
        }
    } else if undo {
        format!("btrfs filesystem defragment -r \"{d}\"")
    } else {
        format!("btrfs filesystem defragment -r -czstd \"{d}\"   # Btrfs only")
    }
}

/// Run compression (Windows). Returns size after.
#[cfg(windows)]
pub fn run(c: &CompactCheck, algo: &str, undo: bool) -> Result<Totals, String> {
    if !ALGORITHMS.contains(&algo) {
        return Err(format!("unknown algorithm '{algo}'"));
    }
    let pattern = format!("{}\\*", display(&c.path));
    let mut cmd = std::process::Command::new("compact");
    if undo {
        cmd.args(["/u", "/s", "/a", "/i", "/exe"]).arg(&pattern);
    } else {
        cmd.args(["/c", "/s", "/a", "/i"])
            .arg(format!("/exe:{algo}"))
            .arg(&pattern);
    }
    let out = cmd
        .stdout(std::process::Stdio::null())
        .status()
        .map_err(|e| format!("could not start compact.exe: {e}"))?;
    if !out.success() {
        return Err(format!("compact.exe exited with {out}"));
    }
    size::measure(&c.path, size::default_threads()).map_err(|e| e.to_string())
}

#[cfg(not(windows))]
pub fn run(c: &CompactCheck, algo: &str, undo: bool) -> Result<Totals, String> {
    Err(format!(
        "agentdisk only runs compression on Windows. On this system run:\n  {}",
        command(&c.path, algo, undo)
    ))
}

pub fn report(before: &Totals, after: &Totals) -> String {
    let saved = before.allocated_bytes.saturating_sub(after.allocated_bytes);
    let ratio = if after.allocated_bytes > 0 {
        before.allocated_bytes as f64 / after.allocated_bytes as f64
    } else {
        1.0
    };
    format!(
        "Before: {}  After: {}  Saved: {} ({ratio:.2}x)\nFiles that are rewritten later become uncompressed again; run compact again to repack them.\n",
        human(before.allocated_bytes),
        human(after.allocated_bytes),
        human(saved)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::temp_dir;

    #[test]
    fn refuses_bad_targets() {
        assert!(check(Path::new("/definitely/missing")).is_err());
        let root = temp_dir("compact");
        std::fs::create_dir_all(root.join("repo/.git/objects")).unwrap();
        assert!(check(&root.join("repo/.git/objects"))
            .unwrap_err()
            .contains(".git"));
        std::fs::write(root.join("f"), b"x").unwrap();
        assert!(check(&root.join("f")).is_err());
        // Fresh folder: changed today.
        assert!(check(&root).unwrap_err().contains("changed"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn commands_mention_path() {
        let c = command(Path::new("/x/sdk"), "xpress8k", false);
        assert!(c.contains("sdk"));
    }
}
