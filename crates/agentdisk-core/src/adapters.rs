//! Agent adapters: small TOML files that say where an agent keeps data and
//! what is safe to touch. Built-in adapters ship inside the binary; users and
//! contributors can add more in `<config_dir>/adapters/*.toml` or with
//! `--adapters <dir>`. Adapters contain data only, never code.
//!
//! ```toml
//! id = "cursor"
//! name = "Cursor"
//! vendor = "Anysphere"
//! docs = ["https://cursor.com/docs/configuration/worktrees"]
//! policy = "Cursor removes worktrees beyond 25 or 50 GB."
//! processes = ["cursor"]
//!
//! [[path]]
//! id = "worktrees"
//! path = "{home}/.cursor/worktrees/*/*"
//! kind = "worktree"
//! class = "user-data"
//! marker = "/.cursor/worktrees/"
//! documented = true
//! ```

use std::fmt;
use std::fs;
use std::path::Path;

/// What deleting something would cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// Rebuilt automatically (caches, logs, build output).
    Regenerable,
    /// History or work the user may want (transcripts, checkpoints).
    UserData,
    /// Never touched (settings, memories, credentials, extensions).
    Protected,
    /// Downloadable but expensive (local AI models).
    Model,
    /// Re-downloadable tool files (SDKs, export templates).
    Download,
}

impl Class {
    pub fn parse(s: &str) -> Option<Class> {
        Some(match s {
            "regenerable" => Class::Regenerable,
            "user-data" => Class::UserData,
            "protected" => Class::Protected,
            "model" => Class::Model,
            "download" => Class::Download,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Class::Regenerable => "regenerable",
            Class::UserData => "user-data",
            Class::Protected => "protected",
            Class::Model => "model",
            Class::Download => "download",
        }
    }
}

impl fmt::Display for Class {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A plain data directory.
    Dir,
    /// Each match is a git worktree (checked with git before any action).
    Worktree,
}

#[derive(Debug, Clone)]
pub struct PathEntry {
    pub id: String,
    pub template: String,
    pub kind: Kind,
    pub class: Class,
    /// Substring (with `/`) that identifies this agent's worktrees.
    pub marker: Option<String>,
    pub documented: bool,
    pub note: Option<String>,
    /// Empty = all platforms; otherwise any of windows/macos/linux.
    pub platforms: Vec<String>,
}

impl PathEntry {
    pub fn applies_here(&self) -> bool {
        self.platforms.is_empty() || self.platforms.iter().any(|p| p == std::env::consts::OS)
    }
}

#[derive(Debug, Clone)]
pub struct Adapter {
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub docs: Vec<String>,
    pub policy: Option<String>,
    pub cleanup: Option<String>,
    pub processes: Vec<String>,
    pub verified: Option<String>,
    pub paths: Vec<PathEntry>,
}

const BUILTIN: &[(&str, &str)] = &[
    ("claude-code", include_str!("../adapters/claude-code.toml")),
    ("cursor", include_str!("../adapters/cursor.toml")),
    ("codex", include_str!("../adapters/codex.toml")),
    ("copilot", include_str!("../adapters/copilot.toml")),
    ("gemini-cli", include_str!("../adapters/gemini-cli.toml")),
    ("cline", include_str!("../adapters/cline.toml")),
    ("roo-code", include_str!("../adapters/roo-code.toml")),
    ("windsurf", include_str!("../adapters/windsurf.toml")),
    ("kiro", include_str!("../adapters/kiro.toml")),
    ("opencode", include_str!("../adapters/opencode.toml")),
    ("amp", include_str!("../adapters/amp.toml")),
    ("aider", include_str!("../adapters/aider.toml")),
    ("zed", include_str!("../adapters/zed.toml")),
    ("pi", include_str!("../adapters/pi.toml")),
    ("ollama", include_str!("../adapters/ollama.toml")),
    ("lmstudio", include_str!("../adapters/lmstudio.toml")),
    ("huggingface", include_str!("../adapters/huggingface.toml")),
    ("godot", include_str!("../adapters/godot.toml")),
];

/// Built-in adapters plus any `*.toml` in the given extra directories.
/// A file with the same `id` as a built-in replaces it.
pub fn load(extra_dirs: &[&Path]) -> (Vec<Adapter>, Vec<String>) {
    let mut out: Vec<Adapter> = Vec::new();
    let mut warnings = Vec::new();
    for (name, src) in BUILTIN {
        match parse_adapter(src) {
            Ok(a) => out.push(a),
            Err(e) => warnings.push(format!("built-in adapter {name}: {e}")),
        }
    }
    for dir in extra_dirs {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        let mut files: Vec<_> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "toml"))
            .collect();
        files.sort();
        for f in files {
            match fs::read_to_string(&f)
                .map_err(|e| e.to_string())
                .and_then(|s| parse_adapter(&s))
            {
                Ok(a) => {
                    out.retain(|b| b.id != a.id);
                    out.push(a);
                }
                Err(e) => warnings.push(format!("{}: {e}", f.display())),
            }
        }
    }
    (out, warnings)
}

/// Parse and validate one adapter file.
pub fn parse_adapter(src: &str) -> Result<Adapter, String> {
    let doc = parse_toml(src)?;
    let s = |k: &str| doc.top_str(k);
    let id = s("id").ok_or("missing 'id'")?;
    if id.is_empty()
        || !id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(format!(
            "bad id '{id}': use lowercase letters, digits and '-'"
        ));
    }
    let mut paths = Vec::new();
    for (table, fields) in &doc.tables {
        if table != "path" {
            return Err(format!("unknown table [[{table}]]"));
        }
        let get = |k: &str| fields.iter().find(|(n, _)| n == k).map(|(_, v)| v);
        let gs = |k: &str| get(k).and_then(TVal::as_str).map(str::to_string);
        let pid = gs("id").ok_or("path without 'id'")?;
        let template = gs("path").ok_or_else(|| format!("path '{pid}' without 'path'"))?;
        let kind = match gs("kind").as_deref().unwrap_or("dir") {
            "dir" => Kind::Dir,
            "worktree" => Kind::Worktree,
            k => return Err(format!("path '{pid}': unknown kind '{k}'")),
        };
        let class_s = gs("class").ok_or_else(|| format!("path '{pid}' without 'class'"))?;
        let class = Class::parse(&class_s)
            .ok_or_else(|| format!("path '{pid}': unknown class '{class_s}'"))?;
        for (k, _) in fields {
            if !matches!(
                k.as_str(),
                "id" | "path" | "kind" | "class" | "marker" | "documented" | "note" | "platforms"
            ) {
                return Err(format!("path '{pid}': unknown key '{k}'"));
            }
        }
        paths.push(PathEntry {
            id: pid,
            template,
            kind,
            class,
            marker: gs("marker"),
            documented: get("documented").and_then(TVal::as_bool).unwrap_or(false),
            note: gs("note"),
            platforms: get("platforms").map(TVal::str_list).unwrap_or_default(),
        });
    }
    for (k, _) in &doc.top {
        if !matches!(
            k.as_str(),
            "id" | "name" | "vendor" | "docs" | "policy" | "cleanup" | "processes" | "verified"
        ) {
            return Err(format!("unknown key '{k}'"));
        }
    }
    Ok(Adapter {
        name: s("name").unwrap_or_else(|| id.clone()),
        vendor: s("vendor").unwrap_or_default(),
        docs: doc.top_get("docs").map(TVal::str_list).unwrap_or_default(),
        policy: s("policy"),
        cleanup: s("cleanup"),
        processes: doc
            .top_get("processes")
            .map(TVal::str_list)
            .unwrap_or_default(),
        verified: s("verified"),
        id,
        paths,
    })
}

// ---------------------------------------------------------------------------
// A tiny TOML subset: `key = value` with strings, booleans, integers and
// arrays of those; `[[table]]` and `[table]` headers; `#` comments.

#[derive(Debug, Clone, PartialEq)]
pub enum TVal {
    Str(String),
    Bool(bool),
    Int(i64),
    Arr(Vec<TVal>),
}

impl TVal {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            TVal::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            TVal::Bool(b) => Some(*b),
            _ => None,
        }
    }
    pub fn as_int(&self) -> Option<i64> {
        match self {
            TVal::Int(i) => Some(*i),
            _ => None,
        }
    }
    pub fn str_list(&self) -> Vec<String> {
        match self {
            TVal::Arr(a) => a
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect(),
            TVal::Str(s) => vec![s.clone()],
            _ => Vec::new(),
        }
    }
}

type Fields = Vec<(String, TVal)>;

#[derive(Debug, Default)]
pub struct TomlDoc {
    pub top: Fields,
    pub tables: Vec<(String, Fields)>,
}

impl TomlDoc {
    pub fn top_get(&self, k: &str) -> Option<&TVal> {
        self.top.iter().find(|(n, _)| n == k).map(|(_, v)| v)
    }
    pub fn top_str(&self, k: &str) -> Option<String> {
        self.top_get(k).and_then(TVal::as_str).map(str::to_string)
    }
}

pub fn parse_toml(src: &str) -> Result<TomlDoc, String> {
    let mut doc = TomlDoc::default();
    let lines: Vec<&str> = src.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let lineno = i + 1;
        let mut line = strip_comment(lines[i]).trim().to_string();
        i += 1;
        if line.is_empty() {
            continue;
        }
        if let Some(name) = line.strip_prefix("[[").and_then(|l| l.strip_suffix("]]")) {
            doc.tables.push((name.trim().to_string(), Vec::new()));
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            doc.tables.push((name.trim().to_string(), Vec::new()));
            continue;
        }
        // Multi-line arrays: keep reading until brackets balance.
        while bracket_depth(&line) > 0 && i < lines.len() {
            line.push(' ');
            line.push_str(strip_comment(lines[i]).trim());
            i += 1;
        }
        let (k, v) = line
            .split_once('=')
            .ok_or_else(|| format!("line {lineno}: expected key = value"))?;
        let key = k.trim().trim_matches('"').to_string();
        if key.is_empty() {
            return Err(format!("line {lineno}: empty key"));
        }
        let mut p = VParser {
            s: v.trim().as_bytes(),
            i: 0,
        };
        let val = p.value().map_err(|e| format!("line {lineno}: {e}"))?;
        p.ws();
        if p.i != p.s.len() {
            return Err(format!("line {lineno}: unexpected text after value"));
        }
        let target = match doc.tables.last_mut() {
            Some((_, f)) => f,
            None => &mut doc.top,
        };
        if target.iter().any(|(n, _)| *n == key) {
            return Err(format!("line {lineno}: duplicate key '{key}'"));
        }
        target.push((key, val));
    }
    Ok(doc)
}

fn strip_comment(line: &str) -> &str {
    let mut in_str = false;
    let mut esc = false;
    for (idx, c) in line.char_indices() {
        if in_str {
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
            }
        } else if c == '"' {
            in_str = true;
        } else if c == '#' {
            return &line[..idx];
        }
    }
    line
}

fn bracket_depth(s: &str) -> i32 {
    let mut d = 0;
    let mut in_str = false;
    let mut esc = false;
    for c in s.chars() {
        if in_str {
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
            }
        } else {
            match c {
                '"' => in_str = true,
                '[' => d += 1,
                ']' => d -= 1,
                _ => {}
            }
        }
    }
    d
}

struct VParser<'a> {
    s: &'a [u8],
    i: usize,
}

impl VParser<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }

    fn value(&mut self) -> Result<TVal, String> {
        self.ws();
        match self.s.get(self.i) {
            Some(b'"') => self.string().map(TVal::Str),
            Some(b'\'') => {
                self.i += 1;
                let start = self.i;
                while self.i < self.s.len() && self.s[self.i] != b'\'' {
                    self.i += 1;
                }
                if self.i >= self.s.len() {
                    return Err("unterminated string".into());
                }
                let v = String::from_utf8_lossy(&self.s[start..self.i]).into_owned();
                self.i += 1;
                Ok(TVal::Str(v))
            }
            Some(b'[') => {
                self.i += 1;
                let mut items = Vec::new();
                loop {
                    self.ws();
                    if self.s.get(self.i) == Some(&b']') {
                        self.i += 1;
                        return Ok(TVal::Arr(items));
                    }
                    items.push(self.value()?);
                    self.ws();
                    match self.s.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b']') => {}
                        _ => return Err("expected ',' or ']' in array".into()),
                    }
                }
            }
            Some(_) => {
                let start = self.i;
                while self.i < self.s.len() && !matches!(self.s[self.i], b',' | b']' | b' ' | b'\t')
                {
                    self.i += 1;
                }
                let word =
                    std::str::from_utf8(&self.s[start..self.i]).map_err(|e| e.to_string())?;
                match word {
                    "true" => Ok(TVal::Bool(true)),
                    "false" => Ok(TVal::Bool(false)),
                    w => w
                        .replace('_', "")
                        .parse::<i64>()
                        .map(TVal::Int)
                        .map_err(|_| format!("unsupported value '{w}'")),
                }
            }
            None => Err("missing value".into()),
        }
    }

    fn string(&mut self) -> Result<String, String> {
        self.i += 1;
        let mut out = String::new();
        let mut buf = Vec::new();
        while self.i < self.s.len() {
            let c = self.s[self.i];
            self.i += 1;
            match c {
                b'"' => {
                    out.push_str(&String::from_utf8_lossy(&buf));
                    return Ok(out);
                }
                b'\\' => {
                    let e = *self.s.get(self.i).ok_or("bad escape")?;
                    self.i += 1;
                    match e {
                        b'"' => buf.push(b'"'),
                        b'\\' => buf.push(b'\\'),
                        b'n' => buf.push(b'\n'),
                        b't' => buf.push(b'\t'),
                        _ => return Err(format!("unsupported escape '\\{}'", e as char)),
                    }
                }
                c => buf.push(c),
            }
        }
        Err("unterminated string".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_builtin_adapters_parse() {
        let (adapters, warnings) = load(&[]);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(adapters.len(), BUILTIN.len());
        for a in &adapters {
            assert!(!a.paths.is_empty(), "{} has no paths", a.id);
            assert!(
                !a.docs.is_empty() || a.paths.iter().all(|p| !p.documented),
                "{}",
                a.id
            );
            for p in &a.paths {
                if p.kind == Kind::Worktree {
                    assert!(p.marker.is_some(), "{}/{} needs a marker", a.id, p.id);
                }
            }
        }
        let ids: std::collections::HashSet<_> = adapters.iter().map(|a| &a.id).collect();
        assert_eq!(ids.len(), adapters.len(), "duplicate ids");
    }

    #[test]
    fn toml_subset() {
        let doc = parse_toml(
            "a = \"x # not a comment\" # comment\nb = true\nc = [\n  \"one\",\n  \"two\", # c\n]\nn = 1_000\n[[path]]\nid = 'p'\n",
        )
        .unwrap();
        assert_eq!(doc.top_str("a").unwrap(), "x # not a comment");
        assert_eq!(doc.top_get("b"), Some(&TVal::Bool(true)));
        assert_eq!(doc.top_get("c").unwrap().str_list(), vec!["one", "two"]);
        assert_eq!(doc.top_get("n").unwrap().as_int(), Some(1000));
        assert_eq!(doc.tables[0].1[0].1.as_str(), Some("p"));
    }

    #[test]
    fn rejects_bad_adapters() {
        assert!(parse_adapter("name = \"x\"").is_err());
        assert!(parse_adapter("id = \"Bad Id\"").is_err());
        assert!(parse_adapter("id = \"x\"\nwhoops = 1").is_err());
        assert!(
            parse_adapter("id = \"x\"\n[[path]]\nid=\"p\"\npath=\"{home}\"\nclass=\"weird\"")
                .is_err()
        );
        assert!(parse_adapter("id = \"x\"\nid = \"y\"").is_err());
    }

    #[test]
    fn user_adapter_overrides_builtin() {
        let dir = crate::testutil::temp_dir("adapters");
        fs::write(
            dir.join("cursor.toml"),
            "id = \"cursor\"\nname = \"My Cursor\"\n[[path]]\nid = \"x\"\npath = \"{home}/nope\"\nclass = \"user-data\"\n",
        )
        .unwrap();
        fs::write(dir.join("broken.toml"), "id = ").unwrap();
        let (adapters, warnings) = load(&[dir.as_path()]);
        assert_eq!(adapters.iter().filter(|a| a.id == "cursor").count(), 1);
        assert_eq!(
            adapters.iter().find(|a| a.id == "cursor").unwrap().name,
            "My Cursor"
        );
        assert_eq!(warnings.len(), 1);
        fs::remove_dir_all(&dir).unwrap();
    }
}
