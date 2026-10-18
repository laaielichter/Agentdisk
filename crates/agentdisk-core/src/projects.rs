//! Walk project folders to find repositories, worktrees and regenerable
//! dependency/build folders (`node_modules`, `target`, `.godot`, ...).
//! A folder only counts when its marker file sits next to it, so a random
//! folder called `target` or `build` is never mistaken for build output.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct EcoRule {
    pub dir: &'static str,
    pub ecosystem: &'static str,
    /// Any of these next to the folder (`*.ext` matches by extension;
    /// `a/b` is a relative path).
    pub markers: &'static [&'static str],
    /// Process name fragment of an editor that must be closed first.
    pub editor: Option<&'static str>,
    /// What rebuilding costs, shown to the user.
    pub rebuild: &'static str,
}

pub const RULES: &[EcoRule] = &[
    EcoRule {
        dir: "node_modules",
        ecosystem: "node",
        markers: &["package.json"],
        editor: None,
        rebuild: "npm/pnpm/yarn install",
    },
    EcoRule {
        dir: ".venv",
        ecosystem: "python",
        markers: &[
            "pyproject.toml",
            "requirements.txt",
            "setup.py",
            "uv.lock",
            "Pipfile",
            "poetry.lock",
        ],
        editor: None,
        rebuild: "uv sync / pip install",
    },
    EcoRule {
        dir: "venv",
        ecosystem: "python",
        markers: &[
            "pyproject.toml",
            "requirements.txt",
            "setup.py",
            "uv.lock",
            "Pipfile",
            "poetry.lock",
        ],
        editor: None,
        rebuild: "uv sync / pip install",
    },
    EcoRule {
        dir: ".tox",
        ecosystem: "python",
        markers: &["tox.ini", "pyproject.toml"],
        editor: None,
        rebuild: "tox",
    },
    EcoRule {
        dir: ".pytest_cache",
        ecosystem: "python",
        markers: &["pyproject.toml", "pytest.ini", "setup.cfg", "tox.ini"],
        editor: None,
        rebuild: "automatic",
    },
    EcoRule {
        dir: ".mypy_cache",
        ecosystem: "python",
        markers: &["pyproject.toml", "mypy.ini", "setup.cfg"],
        editor: None,
        rebuild: "automatic",
    },
    EcoRule {
        dir: ".ruff_cache",
        ecosystem: "python",
        markers: &["pyproject.toml", "ruff.toml", ".ruff.toml"],
        editor: None,
        rebuild: "automatic",
    },
    EcoRule {
        dir: "target",
        ecosystem: "rust",
        markers: &["Cargo.toml"],
        editor: None,
        rebuild: "cargo build",
    },
    EcoRule {
        dir: ".godot",
        ecosystem: "godot",
        markers: &["project.godot"],
        editor: Some("godot"),
        rebuild: "full reimport when the Godot editor opens",
    },
    EcoRule {
        dir: ".import",
        ecosystem: "godot3",
        markers: &["project.godot"],
        editor: Some("godot"),
        rebuild: "full reimport when the Godot editor opens",
    },
    EcoRule {
        dir: "Library",
        ecosystem: "unity",
        markers: &["ProjectSettings/ProjectVersion.txt"],
        editor: Some("unity"),
        rebuild: "Unity recompiles and reimports everything (slow)",
    },
    EcoRule {
        dir: "Intermediate",
        ecosystem: "unreal",
        markers: &["*.uproject"],
        editor: Some("unrealeditor"),
        rebuild: "Unreal rebuilds on next open",
    },
    EcoRule {
        dir: "DerivedDataCache",
        ecosystem: "unreal",
        markers: &["*.uproject"],
        editor: Some("unrealeditor"),
        rebuild: "Unreal rebuilds shaders/assets (slow)",
    },
    EcoRule {
        dir: ".next",
        ecosystem: "node-build",
        markers: &["package.json"],
        editor: None,
        rebuild: "next build",
    },
    EcoRule {
        dir: ".nuxt",
        ecosystem: "node-build",
        markers: &["package.json"],
        editor: None,
        rebuild: "nuxt build",
    },
    EcoRule {
        dir: ".svelte-kit",
        ecosystem: "node-build",
        markers: &["package.json"],
        editor: None,
        rebuild: "automatic",
    },
    EcoRule {
        dir: ".turbo",
        ecosystem: "node-build",
        markers: &["package.json", "turbo.json"],
        editor: None,
        rebuild: "automatic",
    },
    EcoRule {
        dir: ".parcel-cache",
        ecosystem: "node-build",
        markers: &["package.json"],
        editor: None,
        rebuild: "automatic",
    },
    EcoRule {
        dir: ".angular",
        ecosystem: "node-build",
        markers: &["angular.json"],
        editor: None,
        rebuild: "automatic",
    },
    EcoRule {
        dir: ".gradle",
        ecosystem: "gradle",
        markers: &[
            "settings.gradle",
            "settings.gradle.kts",
            "build.gradle",
            "build.gradle.kts",
        ],
        editor: None,
        rebuild: "gradle build",
    },
    EcoRule {
        dir: "build",
        ecosystem: "gradle",
        markers: &["build.gradle", "build.gradle.kts"],
        editor: None,
        rebuild: "gradle build",
    },
    EcoRule {
        dir: "obj",
        ecosystem: "dotnet",
        markers: &["*.csproj", "*.fsproj", "*.vbproj"],
        editor: None,
        rebuild: "dotnet build",
    },
    EcoRule {
        dir: ".zig-cache",
        ecosystem: "zig",
        markers: &["build.zig"],
        editor: None,
        rebuild: "zig build",
    },
    EcoRule {
        dir: "zig-out",
        ecosystem: "zig",
        markers: &["build.zig"],
        editor: None,
        rebuild: "zig build",
    },
];

/// Folder names never descended into while walking.
const SKIP: &[&str] = &[
    ".git",
    "$RECYCLE.BIN",
    "System Volume Information",
    ".Trash",
    ".agentdisk-quarantine",
];

#[derive(Debug, Clone)]
pub struct EcoDir {
    pub path: PathBuf,
    pub rule: &'static EcoRule,
    pub project: PathBuf,
}

#[derive(Debug, Default)]
pub struct ProjectScan {
    /// Folders with a `.git` directory (main worktrees).
    pub repos: Vec<PathBuf>,
    /// Folders with a `.git` file (linked worktrees).
    pub linked: Vec<PathBuf>,
    pub eco: Vec<EcoDir>,
}

fn marker_present(dir: &Path, names: &HashSet<String>, marker: &str) -> bool {
    if let Some(ext) = marker.strip_prefix("*.") {
        names
            .iter()
            .any(|n| n.len() > ext.len() + 1 && n.ends_with(&format!(".{ext}")))
    } else if marker.contains('/') {
        dir.join(marker).is_file()
    } else {
        names.contains(marker)
    }
}

pub fn rule_for(dir: &Path) -> Option<&'static EcoRule> {
    let parent = dir.parent()?;
    let name = dir.file_name()?.to_string_lossy();
    let names: HashSet<String> = fs::read_dir(parent)
        .ok()?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    RULES
        .iter()
        .find(|r| r.dir == name && r.markers.iter().any(|m| marker_present(parent, &names, m)))
}

/// Walk `roots` up to `max_depth` levels deep.
pub fn walk(roots: &[PathBuf], max_depth: usize) -> ProjectScan {
    let mut scan = ProjectScan::default();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut stack: Vec<(PathBuf, usize)> =
        roots.iter().map(|r| (crate::paths::canon(r), 0)).collect();

    while let Some((dir, depth)) = stack.pop() {
        if !seen.insert(dir.clone()) {
            continue;
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        let mut names = HashSet::new();
        let mut subdirs = Vec::new();
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let Ok(ft) = e.file_type() else { continue };
            if name == ".git" {
                if ft.is_dir() {
                    scan.repos.push(dir.clone());
                } else if ft.is_file() {
                    scan.linked.push(dir.clone());
                }
            }
            if ft.is_dir() && !ft.is_symlink() {
                subdirs.push(name.clone());
            }
            names.insert(name);
        }
        subdirs.sort();
        for name in subdirs {
            let child = dir.join(&name);
            if let Some(rule) = RULES.iter().find(|r| {
                r.dir == name && r.markers.iter().any(|m| marker_present(&dir, &names, m))
            }) {
                scan.eco.push(EcoDir {
                    path: child,
                    rule,
                    project: dir.clone(),
                });
                continue;
            }
            if SKIP.contains(&name.as_str()) || name == "node_modules" {
                continue;
            }
            // Hidden folders are skipped, except `.claude` (holds worktrees).
            if name.starts_with('.') && name != ".claude" {
                continue;
            }
            if depth < max_depth {
                stack.push((child, depth + 1));
            }
        }
    }
    scan.repos.sort();
    scan.linked.sort();
    scan.eco.sort_by(|a, b| a.path.cmp(&b.path));
    scan
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::temp_dir;

    #[test]
    fn finds_ecosystem_dirs_only_with_markers() {
        let root = temp_dir("projects");
        let web = root.join("web");
        fs::create_dir_all(web.join("node_modules/lodash")).unwrap();
        fs::write(web.join("package.json"), "{}").unwrap();
        fs::create_dir_all(root.join("notes/target")).unwrap(); // no Cargo.toml
        let game = root.join("game");
        fs::create_dir_all(game.join(".godot/imported")).unwrap();
        fs::write(game.join("project.godot"), "").unwrap();
        let unreal = root.join("ue");
        fs::create_dir_all(unreal.join("Intermediate")).unwrap();
        fs::write(unreal.join("Shooter.uproject"), "{}").unwrap();
        let unity = root.join("unity");
        fs::create_dir_all(unity.join("Library")).unwrap();
        fs::create_dir_all(unity.join("ProjectSettings")).unwrap();
        fs::write(unity.join("ProjectSettings/ProjectVersion.txt"), "").unwrap();
        fs::create_dir_all(root.join("repo/.git")).unwrap();
        fs::create_dir_all(root.join("wt")).unwrap();
        fs::write(root.join("wt/.git"), "gitdir: /x").unwrap();

        let scan = walk(std::slice::from_ref(&root), 6);
        let kinds: Vec<&str> = scan.eco.iter().map(|e| e.rule.dir).collect();
        assert!(kinds.contains(&"node_modules"));
        assert!(kinds.contains(&".godot"));
        assert!(kinds.contains(&"Intermediate"));
        assert!(kinds.contains(&"Library"));
        assert!(
            !kinds.contains(&"target"),
            "target without Cargo.toml must be ignored"
        );
        assert_eq!(scan.repos.len(), 1);
        assert_eq!(scan.linked.len(), 1);
        assert!(rule_for(&crate::paths::canon(&web.join("node_modules"))).is_some());
        assert!(rule_for(&root.join("notes/target")).is_none());
        fs::remove_dir_all(&root).unwrap();
    }
}
