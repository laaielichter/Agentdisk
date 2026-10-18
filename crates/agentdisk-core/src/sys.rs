//! Small platform layer. All Windows calls are plain kernel32 FFI so the
//! project builds without external crates.

use std::fs::Metadata;
use std::path::Path;
use std::process::Command;

/// Identity of a file on disk, used to count hardlinks once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileId {
    pub volume: u64,
    pub index: u64,
}

/// File identity and link count. Cheap on Unix; one handle open on Windows,
/// so callers should only ask when it matters.
#[cfg(unix)]
pub fn file_id(_path: &Path, meta: &Metadata) -> Option<(FileId, u64)> {
    use std::os::unix::fs::MetadataExt;
    Some((
        FileId {
            volume: meta.dev(),
            index: meta.ino(),
        },
        meta.nlink(),
    ))
}

#[cfg(windows)]
pub fn file_id(path: &Path, _meta: &Metadata) -> Option<(FileId, u64)> {
    win::file_info(path).map(|(vol, idx, links)| {
        (
            FileId {
                volume: vol as u64,
                index: idx,
            },
            links as u64,
        )
    })
}

#[cfg(not(any(unix, windows)))]
pub fn file_id(_path: &Path, _meta: &Metadata) -> Option<(FileId, u64)> {
    None
}

/// Bytes a file really uses on disk.
#[cfg(unix)]
pub fn allocated(_path: &Path, meta: &Metadata, _cluster: u64) -> u64 {
    use std::os::unix::fs::MetadataExt;
    meta.blocks() * 512
}

#[cfg(windows)]
pub fn allocated(path: &Path, meta: &Metadata, cluster: u64) -> u64 {
    let raw = win::compressed_size(path).unwrap_or_else(|| meta.len());
    round_up(raw, cluster)
}

#[cfg(not(any(unix, windows)))]
pub fn allocated(_path: &Path, meta: &Metadata, _cluster: u64) -> u64 {
    meta.len()
}

/// Cluster size of the volume holding `path` (Windows only; 0 elsewhere).
pub fn cluster_size(path: &Path) -> u64 {
    #[cfg(windows)]
    {
        win::cluster_size(path).unwrap_or(4096)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        0
    }
}

pub fn round_up(n: u64, unit: u64) -> u64 {
    if unit == 0 || n == 0 {
        n
    } else {
        n.div_ceil(unit) * unit
    }
}

/// An identifier for the volume holding `path`, for same-volume checks.
pub fn volume_of(path: &Path) -> Option<String> {
    let existing = path.ancestors().find(|p| p.exists())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(existing)
            .ok()
            .map(|m| m.dev().to_string())
    }
    #[cfg(windows)]
    {
        win::volume_root(existing).map(|s| s.to_lowercase())
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = existing;
        None
    }
}

/// Free and total bytes on the volume holding `path`.
pub fn disk_space(path: &Path) -> Option<(u64, u64)> {
    let existing = path.ancestors().find(|p| p.exists())?;
    #[cfg(windows)]
    {
        win::free_space(existing)
    }
    #[cfg(not(windows))]
    {
        // POSIX `df -Pk` is on Linux and macOS: 1K-blocks, used, available.
        let out = Command::new("df").arg("-Pk").arg(existing).output().ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        let line = text.lines().nth(1)?;
        let cols: Vec<&str> = line.split_whitespace().collect();
        let total: u64 = cols.get(1)?.parse().ok()?;
        let avail: u64 = cols.get(3)?.parse().ok()?;
        Some((avail * 1024, total * 1024))
    }
}

/// Lower-cased names of running processes (best effort).
pub fn process_names() -> Vec<String> {
    #[cfg(windows)]
    let out = Command::new("tasklist")
        .args(["/fo", "csv", "/nh"])
        .output();
    #[cfg(not(windows))]
    let out = Command::new("ps").args(["-A", "-o", "comm="]).output();
    let Ok(out) = out else { return Vec::new() };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let name = if cfg!(windows) {
                l.split(',').next()?.trim_matches('"').to_string()
            } else {
                l.rsplit('/').next()?.trim().to_string()
            };
            (!name.is_empty()).then(|| name.to_lowercase())
        })
        .collect()
}

/// Processes whose working directory or open files are inside `dir`.
/// Linux only (via /proc); other platforms return an empty list.
pub fn processes_using(dir: &Path) -> Vec<String> {
    #[cfg(target_os = "linux")]
    {
        let mut hits = Vec::new();
        let Ok(procs) = std::fs::read_dir("/proc") else {
            return hits;
        };
        for p in procs.flatten() {
            let pid = p.file_name();
            if !pid.to_string_lossy().bytes().all(|b| b.is_ascii_digit()) {
                continue;
            }
            let base = p.path();
            let mut uses = std::fs::read_link(base.join("cwd")).is_ok_and(|c| c.starts_with(dir));
            if !uses {
                if let Ok(fds) = std::fs::read_dir(base.join("fd")) {
                    uses = fds
                        .flatten()
                        .any(|fd| std::fs::read_link(fd.path()).is_ok_and(|t| t.starts_with(dir)));
                }
            }
            if uses {
                let comm = std::fs::read_to_string(base.join("comm")).unwrap_or_default();
                hits.push(format!("{} (pid {})", comm.trim(), pid.to_string_lossy()));
            }
        }
        hits
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = dir;
        Vec::new()
    }
}

/// Clear read-only flags below `dir` (needed to delete Go module caches and
/// some Windows trees). Never follows links.
pub fn make_writable(dir: &Path) {
    let Ok(meta) = std::fs::symlink_metadata(dir) else {
        return;
    };
    if meta.file_type().is_symlink() {
        return;
    }
    let mut perms = meta.permissions();
    if perms.readonly() {
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        let _ = std::fs::set_permissions(dir, perms);
    }
    if meta.is_dir() {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for e in entries.flatten() {
                make_writable(&e.path());
            }
        }
    }
}

#[cfg(windows)]
mod win {
    use std::ffi::{c_void, OsStr};
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::path::Path;

    #[repr(C)]
    #[derive(Default)]
    #[allow(dead_code)] // filled in by Windows
    struct FileTime {
        lo: u32,
        hi: u32,
    }

    #[repr(C)]
    #[derive(Default)]
    #[allow(dead_code)] // filled in by Windows
    struct ByHandleFileInformation {
        attributes: u32,
        creation: FileTime,
        access: FileTime,
        write: FileTime,
        volume_serial: u32,
        size_high: u32,
        size_low: u32,
        links: u32,
        index_high: u32,
        index_low: u32,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetCompressedFileSizeW(name: *const u16, high: *mut u32) -> u32;
        fn GetLastError() -> u32;
        fn SetLastError(code: u32);
        fn GetVolumePathNameW(name: *const u16, buf: *mut u16, len: u32) -> i32;
        fn GetDiskFreeSpaceW(
            root: *const u16,
            sectors_per_cluster: *mut u32,
            bytes_per_sector: *mut u32,
            free_clusters: *mut u32,
            total_clusters: *mut u32,
        ) -> i32;
        fn GetDiskFreeSpaceExW(
            dir: *const u16,
            free_to_caller: *mut u64,
            total: *mut u64,
            total_free: *mut u64,
        ) -> i32;
        fn CreateFileW(
            name: *const u16,
            access: u32,
            share: u32,
            security: *mut c_void,
            disposition: u32,
            flags: u32,
            template: *mut c_void,
        ) -> isize;
        fn GetFileInformationByHandle(handle: isize, info: *mut ByHandleFileInformation) -> i32;
        fn CloseHandle(handle: isize) -> i32;
    }

    const INVALID_FILE_SIZE: u32 = 0xFFFF_FFFF;
    const INVALID_HANDLE: isize = -1;
    const SHARE_ALL: u32 = 0x1 | 0x2 | 0x4;
    const OPEN_EXISTING: u32 = 3;
    const BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const OPEN_REPARSE_POINT: u32 = 0x0020_0000;

    fn wide(s: &OsStr) -> Vec<u16> {
        s.encode_wide().chain(std::iter::once(0)).collect()
    }

    pub fn compressed_size(path: &Path) -> Option<u64> {
        let name = wide(path.as_os_str());
        let mut high: u32 = 0;
        // SAFETY: valid NUL-terminated name and out-pointer for the call's duration.
        let low = unsafe {
            SetLastError(0);
            GetCompressedFileSizeW(name.as_ptr(), &mut high)
        };
        // SAFETY: reads the calling thread's last-error value.
        if low == INVALID_FILE_SIZE && unsafe { GetLastError() } != 0 {
            return None;
        }
        Some(((high as u64) << 32) | low as u64)
    }

    /// (volume serial, file index, link count) without following reparse points.
    pub fn file_info(path: &Path) -> Option<(u32, u64, u32)> {
        let name = wide(path.as_os_str());
        // SAFETY: valid NUL-terminated name; null security/template are allowed.
        let h = unsafe {
            CreateFileW(
                name.as_ptr(),
                0,
                SHARE_ALL,
                std::ptr::null_mut(),
                OPEN_EXISTING,
                BACKUP_SEMANTICS | OPEN_REPARSE_POINT,
                std::ptr::null_mut(),
            )
        };
        if h == INVALID_HANDLE || h == 0 {
            return None;
        }
        let mut info = ByHandleFileInformation::default();
        // SAFETY: `h` is a valid open handle and `info` a valid out-pointer.
        let ok = unsafe { GetFileInformationByHandle(h, &mut info) };
        // SAFETY: closing the handle we opened above.
        unsafe { CloseHandle(h) };
        if ok == 0 {
            return None;
        }
        let index = ((info.index_high as u64) << 32) | info.index_low as u64;
        Some((info.volume_serial, index, info.links))
    }

    pub fn volume_root(path: &Path) -> Option<String> {
        let abs = std::fs::canonicalize(path).ok()?;
        let name = wide(abs.as_os_str());
        let mut buf = vec![0u16; 1024];
        // SAFETY: buffers are valid for the given lengths.
        let ok = unsafe { GetVolumePathNameW(name.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) };
        if ok == 0 {
            return None;
        }
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(
            std::ffi::OsString::from_wide(&buf[..end])
                .to_string_lossy()
                .into_owned(),
        )
    }

    pub fn cluster_size(path: &Path) -> Option<u64> {
        let root = volume_root(path)?;
        let name = wide(OsStr::new(&root));
        let (mut spc, mut bps, mut free, mut total) = (0u32, 0u32, 0u32, 0u32);
        // SAFETY: NUL-terminated root and valid out-pointers.
        let ok =
            unsafe { GetDiskFreeSpaceW(name.as_ptr(), &mut spc, &mut bps, &mut free, &mut total) };
        if ok == 0 || spc == 0 || bps == 0 {
            return None;
        }
        Some(spc as u64 * bps as u64)
    }

    pub fn free_space(path: &Path) -> Option<(u64, u64)> {
        let name = wide(path.as_os_str());
        let (mut avail, mut total, mut free) = (0u64, 0u64, 0u64);
        // SAFETY: NUL-terminated name and valid out-pointers.
        let ok = unsafe { GetDiskFreeSpaceExW(name.as_ptr(), &mut avail, &mut total, &mut free) };
        (ok != 0).then_some((avail, total))
    }
}
