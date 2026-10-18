//! Read-only, parallel directory size measurement.
//!
//! Per tree we report:
//! - **logical**: sum of file lengths (Explorer "Size").
//! - **allocated**: bytes used on disk (Explorer "Size on disk"), with
//!   hardlinked files counted once. Reflects NTFS/WOF/APFS/Btrfs compression.
//! - **reclaimable**: bytes that would really come back if the tree were
//!   deleted. A hardlinked file only counts when *all* its links are inside
//!   the tree (so `node_modules` linked from a pnpm store shows ~0).
//!
//! Symlinks and junctions are never followed. Nothing is ever written.
//! Known limit: copy-on-write clones (ReFS block cloning, APFS clones,
//! Btrfs reflinks) still count as separate allocations.

use crate::sys::{self, FileId};
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::UNIX_EPOCH;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Totals {
    pub files: u64,
    pub dirs: u64,
    /// Symlinks and junctions seen but not followed.
    pub links_skipped: u64,
    /// Entries that could not be read (permissions, races).
    pub errors: u64,
    pub logical_bytes: u64,
    pub allocated_bytes: u64,
    pub reclaimable_bytes: u64,
    /// Files with more than one hardlink (counted once).
    pub hardlinked_files: u64,
    /// Newest modification time in the tree (seconds since 1970).
    pub newest_mtime: u64,
}

impl Totals {
    pub fn add(&mut self, o: &Totals) {
        self.files += o.files;
        self.dirs += o.dirs;
        self.links_skipped += o.links_skipped;
        self.errors += o.errors;
        self.logical_bytes += o.logical_bytes;
        self.allocated_bytes += o.allocated_bytes;
        self.reclaimable_bytes += o.reclaimable_bytes;
        self.hardlinked_files += o.hardlinked_files;
        self.newest_mtime = self.newest_mtime.max(o.newest_mtime);
    }

    /// Whole days since the newest change, or `None` if unknown.
    pub fn idle_days(&self, now: u64) -> Option<u64> {
        (self.newest_mtime > 0).then(|| now.saturating_sub(self.newest_mtime) / 86_400)
    }
}

#[derive(Default)]
struct Counters {
    files: AtomicU64,
    dirs: AtomicU64,
    links_skipped: AtomicU64,
    errors: AtomicU64,
    logical: AtomicU64,
    allocated: AtomicU64,
    single_link_alloc: AtomicU64,
    newest: AtomicU64,
}

struct LinkEntry {
    seen: u64,
    links: u64,
    alloc: u64,
}

struct Ctx {
    c: Counters,
    links: Mutex<HashMap<FileId, LinkEntry>>,
    cluster: u64,
}

struct Queue {
    state: Mutex<(Vec<PathBuf>, usize)>,
    cv: Condvar,
}

impl Queue {
    fn push(&self, dir: PathBuf) {
        let mut s = self.state.lock().unwrap();
        s.0.push(dir);
        s.1 += 1;
        self.cv.notify_one();
    }

    fn pop(&self) -> Option<PathBuf> {
        let mut s = self.state.lock().unwrap();
        loop {
            if let Some(d) = s.0.pop() {
                return Some(d);
            }
            if s.1 == 0 {
                return None;
            }
            s = self.cv.wait(s).unwrap();
        }
    }

    fn done_one(&self) {
        let mut s = self.state.lock().unwrap();
        s.1 -= 1;
        if s.1 == 0 {
            self.cv.notify_all();
        }
    }
}

pub fn default_threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(2, 16)
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn mtime_secs(meta: &fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Measure `root` read-only. `root` may be a file or a directory.
pub fn measure(root: &Path, threads: usize) -> io::Result<Totals> {
    let meta = fs::symlink_metadata(root)?;
    let ctx = Ctx {
        c: Counters::default(),
        links: Mutex::new(HashMap::new()),
        cluster: sys::cluster_size(root),
    };

    if meta.file_type().is_symlink() {
        ctx.c.links_skipped.fetch_add(1, Ordering::Relaxed);
    } else if !meta.is_dir() {
        count_file(root, &meta, &ctx);
    } else {
        // Absolute (on Windows verbatim `\\?\`) paths avoid the 260-char limit.
        let start = fs::canonicalize(root)?;
        let queue = Queue {
            state: Mutex::new((Vec::new(), 0)),
            cv: Condvar::new(),
        };
        queue.push(start);
        std::thread::scope(|scope| {
            for _ in 0..threads.max(1) {
                scope.spawn(|| {
                    while let Some(dir) = queue.pop() {
                        read_dir(&dir, &queue, &ctx);
                        queue.done_one();
                    }
                });
            }
        });
    }
    Ok(finish(ctx))
}

fn finish(ctx: Ctx) -> Totals {
    let links = ctx.links.into_inner().unwrap();
    let mut shared_reclaim = 0;
    for e in links.values() {
        if e.seen >= e.links {
            shared_reclaim += e.alloc;
        }
    }
    let c = &ctx.c;
    Totals {
        files: c.files.load(Ordering::Relaxed),
        dirs: c.dirs.load(Ordering::Relaxed),
        links_skipped: c.links_skipped.load(Ordering::Relaxed),
        errors: c.errors.load(Ordering::Relaxed),
        logical_bytes: c.logical.load(Ordering::Relaxed),
        allocated_bytes: c.allocated.load(Ordering::Relaxed),
        reclaimable_bytes: c.single_link_alloc.load(Ordering::Relaxed) + shared_reclaim,
        hardlinked_files: links.len() as u64,
        newest_mtime: c.newest.load(Ordering::Relaxed),
    }
}

fn read_dir(dir: &Path, queue: &Queue, ctx: &Ctx) {
    let c = &ctx.c;
    c.dirs.fetch_add(1, Ordering::Relaxed);
    if let Ok(m) = fs::symlink_metadata(dir) {
        c.newest.fetch_max(mtime_secs(&m), Ordering::Relaxed);
    }
    let Ok(entries) = fs::read_dir(dir) else {
        c.errors.fetch_add(1, Ordering::Relaxed);
        return;
    };
    for entry in entries {
        let Ok(entry) = entry else {
            c.errors.fetch_add(1, Ordering::Relaxed);
            continue;
        };
        // `file_type` does not follow links; junctions report as symlinks.
        let Ok(ft) = entry.file_type() else {
            c.errors.fetch_add(1, Ordering::Relaxed);
            continue;
        };
        if ft.is_symlink() {
            c.links_skipped.fetch_add(1, Ordering::Relaxed);
        } else if ft.is_dir() {
            queue.push(entry.path());
        } else {
            let path = entry.path();
            match fs::symlink_metadata(&path) {
                Ok(meta) => count_file(&path, &meta, ctx),
                Err(_) => {
                    c.errors.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }
}

fn count_file(path: &Path, meta: &fs::Metadata, ctx: &Ctx) {
    let c = &ctx.c;
    c.files.fetch_add(1, Ordering::Relaxed);
    c.logical.fetch_add(meta.len(), Ordering::Relaxed);
    c.newest.fetch_max(mtime_secs(meta), Ordering::Relaxed);
    let alloc = sys::allocated(path, meta, ctx.cluster);

    // Only files that can be shared need an identity lookup. On Unix we know
    // the link count up front; on Windows the lookup gives it to us.
    let id = if meta.len() == 0 || (cfg!(unix) && !unix_multi_link(meta)) {
        None
    } else {
        sys::file_id(path, meta)
    };
    match id {
        Some((fid, links)) if links > 1 => {
            let mut map = ctx.links.lock().unwrap();
            let e = map.entry(fid).or_insert(LinkEntry {
                seen: 0,
                links,
                alloc,
            });
            if e.seen == 0 {
                c.allocated.fetch_add(alloc, Ordering::Relaxed);
            }
            e.seen += 1;
        }
        _ => {
            c.allocated.fetch_add(alloc, Ordering::Relaxed);
            c.single_link_alloc.fetch_add(alloc, Ordering::Relaxed);
        }
    }
}

#[cfg(unix)]
fn unix_multi_link(meta: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    meta.nlink() > 1
}

#[cfg(not(unix))]
fn unix_multi_link(_meta: &fs::Metadata) -> bool {
    true
}

/// Human-readable binary size, e.g. `1.50 GiB`.
pub fn human(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{v:.2} {}", UNITS[i])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{fingerprint, temp_dir};

    fn tree(tag: &str) -> PathBuf {
        let root = temp_dir(tag);
        fs::create_dir_all(root.join("a/b/c")).unwrap();
        fs::create_dir_all(root.join("empty")).unwrap();
        fs::write(root.join("one.txt"), vec![b'x'; 1000]).unwrap();
        fs::write(root.join("a/two.bin"), vec![7u8; 10_000]).unwrap();
        fs::write(root.join("a/b/c/three"), vec![0u8; 123_457]).unwrap();
        fs::write(root.join("a/b/zero"), b"").unwrap();
        root
    }

    #[test]
    fn counts_files_dirs_and_logical_bytes() {
        let root = tree("count");
        let t = measure(&root, 4).unwrap();
        assert_eq!(t.files, 4);
        assert_eq!(t.dirs, 5);
        assert_eq!(t.logical_bytes, 1000 + 10_000 + 123_457);
        assert_eq!(t.errors, 0);
        assert!(t.allocated_bytes > 0);
        assert_eq!(t.reclaimable_bytes, t.allocated_bytes);
        assert!(t.newest_mtime > 0);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn same_result_with_one_or_many_threads() {
        let root = tree("threads");
        assert_eq!(measure(&root, 1).unwrap(), measure(&root, 16).unwrap());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn single_file_root() {
        let root = tree("file");
        let t = measure(&root.join("one.txt"), 2).unwrap();
        assert_eq!((t.files, t.dirs, t.logical_bytes), (1, 0, 1000));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn scanning_changes_nothing() {
        let root = tree("readonly");
        let before = fingerprint(&root);
        measure(&root, 4).unwrap();
        assert_eq!(before, fingerprint(&root));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn missing_path_is_an_error() {
        assert!(measure(Path::new("/definitely/not/here/agentdisk"), 2).is_err());
    }

    #[test]
    fn hardlinks_count_once_and_reclaim_only_when_all_links_inside() {
        let root = temp_dir("hardlink");
        let inside = root.join("inside");
        fs::create_dir_all(&inside).unwrap();
        let big = vec![1u8; 200_000];
        fs::write(inside.join("orig"), &big).unwrap();
        for i in 0..100 {
            fs::hard_link(inside.join("orig"), inside.join(format!("link{i}"))).unwrap();
        }
        let t = measure(&inside, 4).unwrap();
        assert_eq!(t.files, 101);
        assert_eq!(t.logical_bytes, 101 * 200_000);
        assert_eq!(t.hardlinked_files, 1);
        assert!(t.allocated_bytes >= 200_000 && t.allocated_bytes < 2 * 200_000 + 65_536);
        assert_eq!(t.reclaimable_bytes, t.allocated_bytes);

        // One extra link outside: deleting `inside` frees nothing.
        fs::hard_link(inside.join("orig"), root.join("outside")).unwrap();
        let t = measure(&inside, 4).unwrap();
        assert_eq!(t.reclaimable_bytes, 0);
        fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_not_followed() {
        let root = tree("symlink");
        std::os::unix::fs::symlink(root.join("a"), root.join("loop")).unwrap();
        let t = measure(&root, 4).unwrap();
        assert_eq!(t.links_skipped, 1);
        assert_eq!(t.files, 4);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn human_sizes() {
        assert_eq!(human(512), "512 B");
        assert_eq!(human(1536), "1.50 KiB");
        assert_eq!(human(3 * 1024 * 1024 * 1024), "3.00 GiB");
    }
}
