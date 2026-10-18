//! Where things live: agentdisk's own data, and path templates used by
//! adapters such as `{home}/.claude` or `{vscode:Cursor}/globalStorage`.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn env_path(var: &str) -> Option<PathBuf> {
    env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

pub fn home() -> PathBuf {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    env_path(var)
        .or_else(|| env_path("HOME"))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn mac_support() -> PathBuf {
    home().join("Library").join("Application Support")
}

/// Roaming app data (Windows `%APPDATA%`), config dir elsewhere.
pub fn appdata() -> PathBuf {
    if cfg!(windows) {
        env_path("APPDATA").unwrap_or_else(|| home().join("AppData").join("Roaming"))
    } else if cfg!(target_os = "macos") {
        mac_support()
    } else {
        xdg_config()
    }
}

/// Local app data (Windows `%LOCALAPPDATA%`), data dir elsewhere.
pub fn localappdata() -> PathBuf {
    if cfg!(windows) {
        env_path("LOCALAPPDATA").unwrap_or_else(|| home().join("AppData").join("Local"))
    } else if cfg!(target_os = "macos") {
        mac_support()
    } else {
        xdg_data()
    }
}

pub fn xdg_data() -> PathBuf {
    if cfg!(windows) {
        return localappdata();
    }
    env_path("XDG_DATA_HOME").unwrap_or_else(|| home().join(".local").join("share"))
}

pub fn xdg_config() -> PathBuf {
    if cfg!(windows) {
        return appdata();
    }
    env_path("XDG_CONFIG_HOME").unwrap_or_else(|| home().join(".config"))
}

pub fn xdg_cache() -> PathBuf {
    if cfg!(windows) {
        return localappdata();
    }
    env_path("XDG_CACHE_HOME").unwrap_or_else(|| home().join(".cache"))
}

/// VS Code-fork user dir, e.g. `%APPDATA%\Cursor\User`.
pub fn vscode_user(app: &str) -> PathBuf {
    let base = if cfg!(windows) {
        appdata()
    } else if cfg!(target_os = "macos") {
        mac_support()
    } else {
        xdg_config()
    };
    base.join(app).join("User")
}

/// agentdisk's own data dir (plans, quarantine journals).
/// `AGENTDISK_HOME` overrides it (used by tests).
pub fn data_dir() -> PathBuf {
    if let Some(p) = env_path("AGENTDISK_HOME") {
        return p;
    }
    if cfg!(target_os = "macos") {
        mac_support().join("agentdisk")
    } else {
        localappdata().join("agentdisk")
    }
}

/// agentdisk's config dir (`config.toml`, extra `adapters/`).
pub fn config_dir() -> PathBuf {
    if let Some(p) = env_path("AGENTDISK_HOME") {
        return p;
    }
    if cfg!(target_os = "macos") {
        mac_support().join("agentdisk")
    } else {
        xdg_config().join("agentdisk")
    }
}

/// Expand one placeholder (without braces). `None` = unknown placeholder.
fn placeholder(name: &str) -> Option<PathBuf> {
    if let Some(rest) = name.strip_prefix("env:") {
        let (var, fallback) = rest.split_once('|').unwrap_or((rest, ""));
        if let Some(v) = env_path(var) {
            return Some(v);
        }
        if fallback.is_empty() {
            return None;
        }
        // Fallback is itself a template like `home/.codex`.
        let mut parts = fallback.split('/');
        let first = parts.next()?;
        let mut p = placeholder(first)?;
        for part in parts {
            p.push(part);
        }
        return Some(p);
    }
    if let Some(app) = name.strip_prefix("vscode:") {
        return Some(vscode_user(app));
    }
    Some(match name {
        "home" => home(),
        "appdata" => appdata(),
        "localappdata" => localappdata(),
        "xdg_data" => xdg_data(),
        "xdg_config" => xdg_config(),
        "xdg_cache" => xdg_cache(),
        _ => return None,
    })
}

/// Expand a template to concrete existing paths. Placeholders are `{...}`;
/// a path segment of exactly `*` matches every child directory.
/// `{repo}` templates are per-repository and expand to nothing here.
pub fn expand(template: &str) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = vec![PathBuf::new()];
    for (i, seg) in split_segments(template).into_iter().enumerate() {
        if i == 0 && seg.is_empty() {
            roots = vec![PathBuf::from("/")];
            continue;
        }
        if i == 0 && seg.len() == 2 && seg.ends_with(':') {
            roots = vec![PathBuf::from(format!("{seg}\\"))];
            continue;
        }
        if seg.is_empty() {
            continue;
        }
        if seg == "*" {
            let mut next = Vec::new();
            for r in &roots {
                if let Ok(entries) = fs::read_dir(r) {
                    let mut kids: Vec<PathBuf> = entries
                        .flatten()
                        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                        .map(|e| e.path())
                        .collect();
                    kids.sort();
                    next.extend(kids);
                }
            }
            roots = next;
        } else if let Some(name) = seg.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
            match placeholder(name) {
                Some(p) if i == 0 => roots = vec![p],
                Some(p) => {
                    for r in &mut roots {
                        r.push(&p);
                    }
                }
                None => return Vec::new(),
            }
        } else {
            for r in &mut roots {
                r.push(seg);
            }
        }
    }
    roots.into_iter().filter(|p| p.exists()).collect()
}

/// Split on `/` but not inside `{...}` placeholders.
fn split_segments(t: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut depth, mut start) = (0i32, 0usize);
    for (i, c) in t.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => depth -= 1,
            '/' if depth == 0 => {
                out.push(&t[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&t[start..]);
    out
}

/// Path with `/` separators, for matching markers across platforms.
pub fn slashed(p: &Path) -> String {
    display(p).replace('\\', "/")
}

/// Display a path without the Windows verbatim prefix
/// (`\\?\C:\x` → `C:\x`, `\\?\UNC\srv\share` → `\\srv\share`).
pub fn display(p: &Path) -> String {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{rest}");
    }
    s.strip_prefix(r"\\?\").unwrap_or(&s).to_string()
}

/// Same as [`display`] but as a path, for tools like git that do not
/// understand verbatim paths.
pub fn plain(p: &Path) -> PathBuf {
    PathBuf::from(display(p))
}

/// Canonical path when possible, otherwise the input.
pub fn canon(p: &Path) -> PathBuf {
    fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::temp_dir;

    #[test]
    fn expands_globs_and_drops_missing() {
        let root = temp_dir("expand");
        fs::create_dir_all(root.join("w/repo1/aaa")).unwrap();
        fs::create_dir_all(root.join("w/repo1/bbb")).unwrap();
        fs::create_dir_all(root.join("w/repo2/ccc")).unwrap();
        fs::write(root.join("w/file"), b"x").unwrap();
        let t = format!("{}/w/*/*", slashed(&root));
        let got = expand(&t);
        assert_eq!(got.len(), 3);
        assert!(expand(&format!("{}/nope/*", slashed(&root))).is_empty());
        assert!(expand("{repo}/.claude/worktrees/*").is_empty());
        assert!(expand("{unknown}/x").is_empty());
        assert_eq!(
            split_segments("{env:A|home/.x}/y/*"),
            vec!["{env:A|home/.x}", "y", "*"]
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn env_placeholder_with_fallback() {
        // Unset variable falls back to the home template.
        let p = placeholder("env:AGENTDISK_SURELY_UNSET_VAR|home/.codex").unwrap();
        assert!(p.ends_with(".codex"));
        assert!(placeholder("env:AGENTDISK_SURELY_UNSET_VAR").is_none());
    }

    #[test]
    fn slashed_strips_verbatim_prefix() {
        assert_eq!(slashed(Path::new(r"\\?\C:\a\b")), "C:/a/b");
        assert_eq!(display(Path::new(r"\\?\C:\a")), r"C:\a");
        assert_eq!(display(Path::new(r"\\?\UNC\srv\share\x")), r"\\srv\share\x");
    }
}
