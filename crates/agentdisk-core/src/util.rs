//! Small helpers: tables, dates, hashing.

use std::path::Path;

/// Render an aligned text table. Columns listed in `right` are right-aligned.
pub fn table(headers: &[&str], rows: &[Vec<String>], right: &[usize]) -> String {
    let n = headers.len();
    let mut w: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
    for r in rows {
        for (i, c) in r.iter().enumerate().take(n) {
            w[i] = w[i].max(c.chars().count());
        }
    }
    let line = |cells: Vec<&str>| {
        let mut s = String::from("  ");
        for (i, c) in cells.iter().enumerate() {
            let pad = w[i].saturating_sub(c.chars().count());
            let last = i + 1 == n;
            if right.contains(&i) {
                s.push_str(&" ".repeat(pad));
                s.push_str(c);
            } else {
                s.push_str(c);
                if !last {
                    s.push_str(&" ".repeat(pad));
                }
            }
            if !last {
                s.push_str("  ");
            }
        }
        s.trim_end().to_string() + "\n"
    };
    let mut out = line(headers.to_vec());
    for r in rows {
        out.push_str(&line(r.iter().map(String::as_str).collect()));
    }
    out
}

/// `YYYY-MM-DD` (UTC) for seconds since 1970.
pub fn date(secs: u64) -> String {
    let (y, m, d) = civil(secs as i64 / 86_400);
    format!("{y:04}-{m:02}-{d:02}")
}

/// `YYYY-MM-DD HH:MM UTC`.
pub fn datetime(secs: u64) -> String {
    let rem = secs % 86_400;
    format!(
        "{} {:02}:{:02} UTC",
        date(secs),
        rem / 3600,
        (rem % 3600) / 60
    )
}

fn civil(days: i64) -> (i64, u32, u32) {
    // Howard Hinnant's days-to-civil algorithm.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// "3d", "5h" style age.
pub fn age(days: Option<u64>) -> String {
    match days {
        Some(0) => "today".into(),
        Some(d) => format!("{d}d"),
        None => "?".into(),
    }
}

/// FNV-1a 64-bit hash (stable across runs and platforms).
pub fn fnv64(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in data {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

pub fn fnv_file(path: &Path) -> Option<u64> {
    std::fs::read(path).ok().map(|d| fnv64(&d))
}

/// Short unique-ish id from time and process.
pub fn new_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let h = fnv64(format!("{nanos}-{}", std::process::id()).as_bytes());
    format!(
        "{}-{:06x}",
        date((nanos / 1_000_000_000) as u64).replace('-', ""),
        h & 0xff_ffff
    )
}

/// Shorten long paths for tables: keep the end.
pub fn short_path(p: &str, max: usize) -> String {
    let n = p.chars().count();
    if n <= max {
        return p.to_string();
    }
    let tail: String = p.chars().skip(n - (max - 1)).collect();
    format!("…{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(1_792_281_600), "2026-10-18");
        assert_eq!(date(951_782_400), "2000-02-29");
        assert_eq!(datetime(3_600 * 25 + 60), "1970-01-02 01:01 UTC");
    }

    #[test]
    fn table_aligns() {
        let t = table(
            &["a", "size"],
            &[vec!["long name".into(), "1 B".into()]],
            &[1],
        );
        let lines: Vec<&str> = t.lines().collect();
        assert_eq!(lines[0], "  a          size");
        assert_eq!(lines[1], "  long name   1 B");
    }

    #[test]
    fn hashing_and_ids() {
        assert_eq!(fnv64(b""), 0xcbf2_9ce4_8422_2325);
        assert_ne!(fnv64(b"a"), fnv64(b"b"));
        assert!(new_id().len() > 10);
        assert_eq!(short_path("abcdef", 4), "…def");
    }
}
