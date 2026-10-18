//! Helpers shared by unit tests.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A fresh, empty temporary directory.
pub fn temp_dir(tag: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "agentdisk-{tag}-{}-{nanos}-{n}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Recursive listing with sizes and mtimes, to prove nothing changed.
pub fn fingerprint(p: &Path) -> Vec<String> {
    let mut out = Vec::new();
    walk(p, &mut out);
    out
}

fn walk(p: &Path, out: &mut Vec<String>) {
    let mut entries: Vec<_> = fs::read_dir(p)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for e in entries {
        let m = fs::symlink_metadata(&e).unwrap();
        out.push(format!(
            "{} {} {:?}",
            e.display(),
            m.len(),
            m.modified().ok()
        ));
        if m.is_dir() {
            walk(&e, out);
        }
    }
}
