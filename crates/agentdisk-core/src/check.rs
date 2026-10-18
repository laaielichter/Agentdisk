//! `agentdisk check`: a fast health check for hooks (no deep scan).

use crate::adapters::Kind;
use crate::json::Value;
use crate::paths;
use crate::scan;
use crate::size::human;
use crate::sys;

#[derive(Debug)]
pub struct Health {
    pub free: Option<u64>,
    pub total: Option<u64>,
    pub worktrees: Vec<(String, usize)>,
    pub warnings: Vec<String>,
}

pub fn run(min_free_gb: u64, max_worktrees: usize) -> Health {
    let (adapters, _) = scan::load_adapters(&[]);
    let disk = sys::disk_space(&paths::home());
    let mut per_agent = Vec::new();
    for a in &adapters {
        let n: usize = a
            .paths
            .iter()
            .filter(|p| p.kind == Kind::Worktree && p.applies_here())
            .map(|p| paths::expand(&p.template).len())
            .sum();
        if n > 0 {
            per_agent.push((a.name.clone(), n));
        }
    }
    let mut warnings = Vec::new();
    if let Some((free, _)) = disk {
        if free < min_free_gb * 1024 * 1024 * 1024 {
            warnings.push(format!(
                "Low disk space: {} free. Run `agentdisk scan` to see what agents left behind.",
                human(free)
            ));
        }
    }
    let total: usize = per_agent.iter().map(|x| x.1).sum();
    if total > max_worktrees {
        warnings.push(format!(
            "{total} agent worktrees on disk. Run `agentdisk plan` to see which are safe to clean."
        ));
    }
    Health {
        free: disk.map(|d| d.0),
        total: disk.map(|d| d.1),
        worktrees: per_agent,
        warnings,
    }
}

pub fn render(h: &Health) -> String {
    let mut out = String::new();
    if h.warnings.is_empty() {
        out.push_str("agentdisk: OK");
        if let Some(f) = h.free {
            out.push_str(&format!(" ({} free)", human(f)));
        }
        out.push('\n');
    } else {
        for w in &h.warnings {
            out.push_str(&format!("agentdisk: {w}\n"));
        }
    }
    out
}

pub fn to_json(h: &Health) -> Value {
    Value::obj(vec![
        ("free_bytes", h.free.into()),
        ("total_bytes", h.total.into()),
        (
            "agent_worktrees",
            Value::Obj(
                h.worktrees
                    .iter()
                    .map(|(n, c)| (n.clone(), Value::from(*c)))
                    .collect(),
            ),
        ),
        (
            "warnings",
            h.warnings
                .iter()
                .map(|w| Value::from(w.as_str()))
                .collect::<Vec<_>>()
                .into(),
        ),
    ])
}
