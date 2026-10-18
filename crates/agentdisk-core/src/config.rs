//! `config.toml` in the config dir. Everything is optional.
//!
//! ```toml
//! projects = ["C:/Users/me/Projects", "D:/games"]
//! worktree_days = 7
//! build_days = 14
//! max_depth = 6
//! adapters = ["C:/Users/me/agentdisk-adapters"]
//! ```

use crate::adapters::{parse_toml, TVal};
use crate::paths;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    pub projects: Vec<PathBuf>,
    pub worktree_days: u64,
    pub build_days: u64,
    pub max_depth: usize,
    pub adapters: Vec<PathBuf>,
    pub warnings: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            projects: Vec::new(),
            worktree_days: 7,
            build_days: 14,
            max_depth: 6,
            adapters: Vec::new(),
            warnings: Vec::new(),
        }
    }
}

pub fn path() -> PathBuf {
    paths::config_dir().join("config.toml")
}

pub fn load() -> Config {
    let mut c = Config::default();
    let Ok(text) = std::fs::read_to_string(path()) else {
        return c;
    };
    match parse_toml(&text) {
        Err(e) => c.warnings.push(format!("config.toml: {e}")),
        Ok(doc) => {
            for (k, v) in &doc.top {
                let num = |v: &TVal| v.as_int().filter(|n| *n >= 0).map(|n| n as u64);
                match k.as_str() {
                    "projects" => {
                        c.projects = v.str_list().into_iter().map(PathBuf::from).collect()
                    }
                    "adapters" => {
                        c.adapters = v.str_list().into_iter().map(PathBuf::from).collect()
                    }
                    "worktree_days" => c.worktree_days = num(v).unwrap_or(c.worktree_days),
                    "build_days" => c.build_days = num(v).unwrap_or(c.build_days),
                    "max_depth" => c.max_depth = num(v).map(|n| n as usize).unwrap_or(c.max_depth),
                    other => c
                        .warnings
                        .push(format!("config.toml: unknown key '{other}'")),
                }
            }
        }
    }
    c
}

pub const EXAMPLE: &str = r#"# agentdisk config
# Folders that hold your repositories (scanned for worktrees and build folders).
projects = []
# Days without changes before something may be planned for cleanup.
worktree_days = 7
build_days = 14
# How deep to look inside project folders.
max_depth = 6
# Extra folders with adapter .toml files.
adapters = []
"#;
