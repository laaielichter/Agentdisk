//! agentdisk command line.

use agentdisk_core::cleanup::{self, PlanOptions};
use agentdisk_core::json::Value;
use agentdisk_core::paths::display;
use agentdisk_core::scan::{self, ScanOptions};
use agentdisk_core::size::{self, human};
use agentdisk_core::{adapters, advise, check, compact, config, explain, mcp, VERSION};
use std::collections::HashMap;
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Debug)]
enum CliError {
    Message(String),
    Output(io::Error),
}

impl From<String> for CliError {
    fn from(message: String) -> Self {
        Self::Message(message)
    }
}

impl From<io::Error> for CliError {
    fn from(error: io::Error) -> Self {
        Self::Output(error)
    }
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Message(message) => message.fmt(f),
            Self::Output(error) => error.fmt(f),
        }
    }
}

fn write_stdout(args: std::fmt::Arguments<'_>) -> Result<(), CliError> {
    let mut stdout = io::stdout().lock();
    stdout.write_fmt(args)?;
    stdout.flush()?;
    Ok(())
}

macro_rules! out {
    ($($arg:tt)*) => {
        write_stdout(format_args!($($arg)*))
    };
}

macro_rules! outln {
    ($($arg:tt)*) => {
        write_stdout(format_args!("{}\n", format_args!($($arg)*)))
    };
}

const HELP: &str = "\
agentdisk — the honest disk report for AI coding agents

USAGE:
    agentdisk <command> [options]

LOOK (read-only):
    scan        What every agent left on disk (agent data, worktrees, build folders)
    size <dir>  Measure one folder (size, on disk, freed if deleted)
    explain <dir>  What is this folder, who made it, may it be cleaned?
    advise      Ways to need less space (shared stores, duplicates, caches)
    check       Fast check of free space and agent worktrees (for hooks)
    adapters    List the agents agentdisk knows and where they keep data
    plans       List saved cleanup plans and whether they were applied
    quarantine  Show what earlier cleanups moved aside

CLEAN UP (always reversible first):
    plan             Make a cleanup plan (moves nothing)
    apply <id>       Move the plan's items to quarantine (human only, asks a code)
    restore <id>     Put a plan's items back
    purge            Delete quarantined items for good (default: older than 7 days)
    compact <dir>    Compress a mostly-read folder (Windows WOF; prints command elsewhere)

OTHER:
    mcp         Run as an MCP server on stdio (read-only tools + plan)
    config      Show config path (--init writes an example)

COMMON OPTIONS:
    -p, --projects <dir>   Folder with repositories (repeatable; or set in config)
    --json                 Machine-readable output
    -V, --version          Print version
    -h, --help             Print this help

Run `agentdisk <command> --help` for details.";

struct Args {
    pos: Vec<String>,
    flags: HashMap<String, Vec<String>>,
}

impl Args {
    fn has(&self, f: &str) -> bool {
        self.flags.contains_key(f)
    }
    fn one(&self, f: &str) -> Option<&str> {
        self.flags.get(f).and_then(|v| v.last()).map(String::as_str)
    }
    fn many(&self, f: &str) -> Vec<String> {
        self.flags.get(f).cloned().unwrap_or_default()
    }
    fn num(&self, f: &str) -> Result<Option<u64>, String> {
        self.one(f)
            .map(|v| {
                v.parse::<u64>()
                    .map_err(|_| format!("--{f} needs a number, got '{v}'"))
            })
            .transpose()
    }
}

/// Parse `rest` given flags that take a value and flags that don't.
fn parse(rest: &[String], valued: &[&str], switches: &[&str]) -> Result<Args, String> {
    let mut a = Args {
        pos: Vec::new(),
        flags: HashMap::new(),
    };
    let alias = |s: &str| match s {
        "-p" => "projects".to_string(),
        "-h" => "help".to_string(),
        other => other.trim_start_matches("--").to_string(),
    };
    let mut i = 0;
    while i < rest.len() {
        let arg = &rest[i];
        if arg.starts_with('-') && arg.len() > 1 {
            let (name, inline) = match arg.split_once('=') {
                Some((n, v)) => (alias(n), Some(v.to_string())),
                None => (alias(arg), None),
            };
            if valued.contains(&name.as_str()) {
                let v = match inline {
                    Some(v) => v,
                    None => {
                        i += 1;
                        rest.get(i)
                            .cloned()
                            .ok_or(format!("--{name} needs a value"))?
                    }
                };
                a.flags.entry(name).or_default().push(v);
            } else if switches.contains(&name.as_str()) || name == "help" {
                a.flags.entry(name).or_default();
            } else {
                return Err(format!("unknown option '{arg}'"));
            }
        } else {
            a.pos.push(arg.clone());
        }
        i += 1;
    }
    Ok(a)
}

fn no_positionals(a: &Args, hint: Option<&str>) -> Result<(), String> {
    if let Some(arg) = a.pos.first() {
        let mut message = format!("unexpected argument '{arg}'");
        if let Some(flag) = hint {
            message.push_str(&format!(". Did you mean `{flag} {arg}`?"));
        }
        return Err(message);
    }
    Ok(())
}

fn scan_options(a: &Args, cfg: &config::Config) -> Result<ScanOptions, String> {
    let mut projects: Vec<PathBuf> = a.many("projects").into_iter().map(PathBuf::from).collect();
    if projects.is_empty() {
        projects = cfg.projects.clone();
    }
    let mut adapter_dirs = cfg.adapters.clone();
    adapter_dirs.extend(a.many("adapters").into_iter().map(PathBuf::from));
    Ok(ScanOptions {
        projects,
        adapter_dirs,
        threads: a
            .num("threads")?
            .map(|n| n.max(1) as usize)
            .unwrap_or_else(size::default_threads),
        max_depth: a.num("depth")?.map(|n| n as usize).unwrap_or(cfg.max_depth),
        agents: !a.has("no-agents"),
    })
}

const SCAN_FLAGS: &[&str] = &["projects", "adapters", "threads", "depth"];

fn print_warnings(cfg: &config::Config) {
    for w in &cfg.warnings {
        eprintln!("warning: {w}");
    }
}

/// Ask a human to type `code`. Refuses when stdin is not a terminal, so
/// scripts and AI agents cannot confirm on their own.
fn human_confirms(what: &str, code: &str) -> Result<bool, String> {
    if !std::io::stdin().is_terminal() {
        return Err(format!(
            "{what} must be confirmed by a person in an interactive terminal.\nOpen a terminal yourself and run the same command there."
        ));
    }
    eprint!("Type {code} to confirm {what} (anything else cancels): ");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    Ok(line.trim() == code)
}

fn cmd_scan(rest: &[String]) -> Result<ExitCode, CliError> {
    let a = parse(rest, SCAN_FLAGS, &["json", "no-agents"])?;
    if a.has("help") {
        outln!("agentdisk scan [-p <dir>]... [--json] [--depth N] [--threads N] [--adapters <dir>] [--no-agents]\n\nRead-only report of agent data, worktrees and dependency/build folders.\nWithout -p it scans agent folders only (plus `projects` from config).")?;
        return Ok(ExitCode::SUCCESS);
    }
    no_positionals(&a, Some("-p"))?;
    let cfg = config::load();
    print_warnings(&cfg);
    let r = scan::run(&scan_options(&a, &cfg)?);
    if a.has("json") {
        outln!("{}", scan::to_json(&r).pretty())?;
    } else {
        out!("{}", scan::render(&r))?;
    }
    Ok(ExitCode::SUCCESS)
}

fn cmd_explain(rest: &[String]) -> Result<ExitCode, CliError> {
    let a = parse(rest, &[], &["json"])?;
    if a.has("help") || a.pos.is_empty() {
        outln!("agentdisk explain <dir>... [--json]\n\nRead-only. Says what a folder is (agent data, worktree, build folder),\nwho made it, and whether `agentdisk plan` would clean it, and why.\n\nSeveral folders: --json prints one array in the order given. A folder that\ncannot be read is reported ({{\"path\", \"error\"}} in JSON, stderr in text), the\nrest still run, and the exit code is 1.")?;
        return Ok(if a.pos.is_empty() && !a.has("help") {
            ExitCode::from(2)
        } else {
            ExitCode::SUCCESS
        });
    }
    each_folder(&a, |p| {
        let e = explain::explain(Path::new(p))?;
        Ok((explain::to_json(&e), explain::render(&e)))
    })
}

/// Run `one` (JSON and text output for a folder) over every folder given.
/// One folder: its output as before, and an error ends the run (exit 2).
/// Several: `--json` prints one array in the order given; a folder that
/// fails becomes `{"path", "error"}` (text: an error on stderr) and the
/// rest still run, then the exit code is 1.
fn each_folder(
    a: &Args,
    one: impl Fn(&str) -> Result<(Value, String), String>,
) -> Result<ExitCode, CliError> {
    let json = a.has("json");
    if let [p] = a.pos.as_slice() {
        let (v, text) = one(p)?;
        if json {
            outln!("{v}")?;
        } else {
            out!("{text}")?;
        }
        return Ok(ExitCode::SUCCESS);
    }
    let (mut all, mut failed) = (Vec::new(), false);
    for p in &a.pos {
        match one(p) {
            Ok((v, _)) if json => all.push(v),
            Ok((_, text)) => out!("{text}")?,
            Err(e) => {
                failed = true;
                if json {
                    all.push(Value::obj(vec![
                        ("path", p.as_str().into()),
                        ("error", e.into()),
                    ]));
                } else {
                    eprintln!("error: {e}");
                }
            }
        }
    }
    if json {
        outln!("{}", Value::Arr(all))?;
    }
    Ok(ExitCode::from(failed as u8))
}

fn cmd_size(rest: &[String]) -> Result<ExitCode, CliError> {
    let a = parse(rest, &["threads"], &["json"])?;
    if a.has("help") || a.pos.is_empty() {
        outln!("agentdisk size <dir>... [--json] [--threads N]\n\nMeasure folders. Read-only. Symlinks and junctions are not followed;\nhardlinked files are counted once.\n\nSeveral folders: --json prints one array in the order given. A folder that\ncannot be read is reported ({{\"path\", \"error\"}} in JSON, stderr in text), the\nrest still run, and the exit code is 1.")?;
        return Ok(if a.pos.is_empty() && !a.has("help") {
            ExitCode::from(2)
        } else {
            ExitCode::SUCCESS
        });
    }
    let threads = a
        .num("threads")?
        .map(|n| n.max(1) as usize)
        .unwrap_or_else(size::default_threads);
    each_folder(&a, |p| {
        let t = size::measure(Path::new(p), threads).map_err(|e| format!("{p}: {e}"))?;
        let v = Value::obj(vec![
            ("path", p.into()),
            ("files", t.files.into()),
            ("dirs", t.dirs.into()),
            ("logical_bytes", t.logical_bytes.into()),
            ("allocated_bytes", t.allocated_bytes.into()),
            ("reclaimable_bytes", t.reclaimable_bytes.into()),
            ("hardlinked_files", t.hardlinked_files.into()),
            ("links_skipped", t.links_skipped.into()),
            ("errors", t.errors.into()),
            ("newest_mtime", t.newest_mtime.into()),
        ]);
        let mut s = format!(
            "{p}\n  files {}   folders {}   links skipped {}   errors {}\n",
            t.files, t.dirs, t.links_skipped, t.errors
        );
        for (label, bytes) in [
            ("size:             ", t.logical_bytes),
            ("on disk:          ", t.allocated_bytes),
            ("freed if deleted: ", t.reclaimable_bytes),
        ] {
            s.push_str(&format!(
                "  {label} {:>12}  ({bytes} bytes)\n",
                human(bytes)
            ));
        }
        if t.hardlinked_files > 0 {
            s.push_str(&format!(
                "  {} hardlinked file(s), counted once\n",
                t.hardlinked_files
            ));
        }
        Ok((v, s))
    })
}

fn cmd_advise(rest: &[String]) -> Result<ExitCode, CliError> {
    let a = parse(rest, SCAN_FLAGS, &["json", "no-agents"])?;
    if a.has("help") {
        outln!("agentdisk advise [-p <dir>]... [--json] [--depth N] [--threads N]\n                 [--adapters <dir>] [--no-agents]\n\nSuggestions only; agentdisk never runs them.")?;
        return Ok(ExitCode::SUCCESS);
    }
    no_positionals(&a, Some("-p"))?;
    let cfg = config::load();
    print_warnings(&cfg);
    let r = scan::run(&scan_options(&a, &cfg)?);
    let list = advise::advise(&r);
    if a.has("json") {
        outln!("{}", advise::to_json(&list).pretty())?;
    } else {
        out!("{}", advise::render(&list))?;
    }
    Ok(ExitCode::SUCCESS)
}

fn cmd_plan(rest: &[String]) -> Result<ExitCode, CliError> {
    let mut valued = SCAN_FLAGS.to_vec();
    valued.extend(["worktree-days", "build-days"]);
    let a = parse(
        rest,
        &valued,
        &[
            "json",
            "no-agents",
            "no-worktrees",
            "no-build",
            "no-agent-caches",
        ],
    )?;
    if a.has("help") {
        outln!("agentdisk plan [-p <dir>]... [--worktree-days N] [--build-days N]\n                [--no-worktrees] [--no-build] [--no-agent-caches] [--json]\n                [--depth N] [--threads N] [--adapters <dir>] [--no-agents]\n\nMakes and saves a cleanup plan. Moves nothing. A person applies it with\n`agentdisk apply <id>` in a terminal.")?;
        return Ok(ExitCode::SUCCESS);
    }
    no_positionals(&a, Some("-p"))?;
    let cfg = config::load();
    print_warnings(&cfg);
    let opts = PlanOptions {
        scan: scan_options(&a, &cfg)?,
        worktree_days: a.num("worktree-days")?.unwrap_or(cfg.worktree_days),
        build_days: a.num("build-days")?.unwrap_or(cfg.build_days),
        worktrees: !a.has("no-worktrees"),
        build: !a.has("no-build"),
        agent_caches: !a.has("no-agent-caches"),
    };
    let (plan, _) = cleanup::plan_now(&opts);
    let path = cleanup::save(&plan).map_err(|e| format!("could not save plan: {e}"))?;
    if a.has("json") {
        outln!("{}", cleanup::plan_to_json(&plan).pretty())?;
    } else {
        out!("{}", cleanup::render_plan(&plan, Some(&path)))?;
    }
    Ok(ExitCode::SUCCESS)
}

fn cmd_plans(rest: &[String]) -> Result<ExitCode, CliError> {
    let a = parse(rest, &[], &["json"])?;
    if a.has("help") {
        outln!("agentdisk plans [--json]\n\nRead-only. Lists the saved cleanup plans, newest first, with what each\nwould free and whether it was applied.")?;
        return Ok(ExitCode::SUCCESS);
    }
    no_positionals(&a, None)?;
    let list = cleanup::list_plans();
    if a.has("json") {
        outln!("{}", cleanup::plans_to_json(&list).pretty())?;
    } else {
        out!("{}", cleanup::render_plans(&list))?;
    }
    Ok(ExitCode::SUCCESS)
}

fn cmd_apply(rest: &[String]) -> Result<ExitCode, CliError> {
    let a = parse(rest, &[], &["dry-run"])?;
    if a.has("help") || a.pos.is_empty() {
        outln!("agentdisk apply <plan-id|latest> [--dry-run]\n\nRe-checks every item, then moves it to quarantine. Nothing is deleted.\nMust be run by a person in an interactive terminal.")?;
        return Ok(if a.pos.is_empty() && !a.has("help") {
            ExitCode::from(2)
        } else {
            ExitCode::SUCCESS
        });
    }
    let plan = cleanup::load(&a.pos[0])?;
    if plan.items.is_empty() {
        outln!("Plan {} has nothing to apply.", plan.id)?;
        return Ok(ExitCode::SUCCESS);
    }
    let dry = a.has("dry-run");
    if !dry {
        outln!(
            "Plan {}: {} item(s), up to {}.",
            plan.id,
            plan.items.len(),
            human(plan.total())
        )?;
        for i in &plan.items {
            outln!(
                "  {:>10}  {}  {}",
                human(i.bytes),
                i.label,
                display(&i.path)
            )?;
        }
        outln!(
            "Items are moved to quarantine, not deleted. `agentdisk restore {}` undoes it.",
            plan.id
        )?;
        if !human_confirms("moving these items to quarantine", &plan.code())? {
            outln!("Cancelled. Nothing changed.")?;
            return Ok(ExitCode::SUCCESS);
        }
    }
    let out = cleanup::apply(&plan, dry);
    let verb = if dry { "Would move" } else { "Moved" };
    let total: u64 = out.moved.iter().map(|m| m.bytes).sum();
    outln!("{verb} {} item(s), {}:", out.moved.len(), human(total))?;
    for m in &out.moved {
        outln!("  {}  →  {}", display(&m.original), display(&m.quarantine))?;
    }
    if !out.skipped.is_empty() {
        outln!("Skipped {} item(s):", out.skipped.len())?;
        for (p, why) in &out.skipped {
            outln!("  {}: {why}", display(p))?;
        }
    }
    if !dry && !out.moved.is_empty() {
        outln!(
            "\nSpace is freed for good after `agentdisk purge` (items older than 7 days).\nUndo now: agentdisk restore {}",
            plan.id
        )?;
    }
    Ok(ExitCode::SUCCESS)
}

fn cmd_restore(rest: &[String]) -> Result<ExitCode, CliError> {
    let a = parse(rest, &[], &[])?;
    if a.has("help") || a.pos.is_empty() {
        outln!("agentdisk restore <plan-id>\n\nMoves a plan's quarantined items back to where they were.")?;
        return Ok(if a.pos.is_empty() && !a.has("help") {
            ExitCode::from(2)
        } else {
            ExitCode::SUCCESS
        });
    }
    cleanup::check_plan_id(&a.pos[0])?;
    let (done, problems) = cleanup::restore(&a.pos[0]);
    outln!("Restored {} item(s).", done.len())?;
    for d in &done {
        outln!("  {}", display(&d.original))?;
    }
    for p in &problems {
        eprintln!("  problem: {p}");
    }
    Ok(if problems.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

fn cmd_purge(rest: &[String]) -> Result<ExitCode, CliError> {
    let a = parse(rest, &["older-than", "plan"], &["dry-run"])?;
    if a.has("help") {
        outln!("agentdisk purge [--older-than DAYS (default 7)] [--plan <id>] [--dry-run]\n\nDeletes quarantined items for good. Must be run by a person in a terminal.")?;
        return Ok(ExitCode::SUCCESS);
    }
    no_positionals(&a, Some("--plan"))?;
    let days = a.num("older-than")?.unwrap_or(7);
    let (list, refused, quarantined) = cleanup::purge_candidates(a.one("plan"), days)?;
    let status = |problems: bool| {
        if problems {
            ExitCode::from(1)
        } else {
            ExitCode::SUCCESS
        }
    };
    for p in &refused {
        eprintln!("  problem: {p}");
    }
    if list.is_empty() {
        if refused.is_empty() {
            match a.one("plan") {
                Some(plan) if quarantined == 0 => {
                    outln!("Nothing in quarantine for plan '{plan}'. See the plans with: agentdisk quarantine")?;
                    return Ok(ExitCode::from(1));
                }
                Some(plan) => outln!("Plan {plan} has {quarantined} item(s) in quarantine, none older than {days} day(s). Use --older-than 0 to delete them now.")?,
                None => outln!("Nothing in quarantine is older than {days} day(s).")?,
            }
        } else {
            outln!("Nothing was deleted.")?;
        }
        return Ok(status(!refused.is_empty()));
    }
    let total: u64 = list.iter().map(|(_, e)| e.bytes).sum();
    outln!(
        "{} item(s), {}, will be deleted for good:",
        list.len(),
        human(total)
    )?;
    for (_, e) in &list {
        outln!(
            "  {}  (was {})",
            display(&e.quarantine),
            display(&e.original)
        )?;
    }
    if a.has("dry-run") {
        return Ok(status(!refused.is_empty()));
    }
    if !human_confirms("permanent deletion", "delete")? {
        outln!("Cancelled. Nothing changed.")?;
        return Ok(ExitCode::SUCCESS);
    }
    let (freed, problems) = cleanup::purge(&list);
    outln!("Freed {}.", human(freed))?;
    for p in &problems {
        eprintln!("  problem: {p}");
    }
    Ok(status(!problems.is_empty() || !refused.is_empty()))
}

fn cmd_compact(rest: &[String]) -> Result<ExitCode, CliError> {
    let a = parse(rest, &["algo"], &["undo", "dry-run"])?;
    if a.has("help") || a.pos.is_empty() {
        outln!(
            "agentdisk compact <dir> [--algo xpress4k|xpress8k|xpress16k|lzx] [--undo] [--dry-run]\n\nTransparent compression for folders that are mostly read (SDKs, engine installs,\nold projects). Expect 1.3-2x. Refuses .git, node_modules, .venv, .godot,\nshared package stores and folders changed in the last {} days.",
            compact::MIN_IDLE_DAYS
        )?;
        return Ok(if a.pos.is_empty() && !a.has("help") {
            ExitCode::from(2)
        } else {
            ExitCode::SUCCESS
        });
    }
    let algo = a.one("algo").unwrap_or("xpress8k");
    if !compact::ALGORITHMS.contains(&algo) {
        return Err(format!(
            "unknown --algo '{algo}' (use {})",
            compact::ALGORITHMS.join(", ")
        )
        .into());
    }
    let undo = a.has("undo");
    let c = compact::check(Path::new(&a.pos[0]))?;
    outln!(
        "{}: {} on disk",
        display(&c.path),
        human(c.before.allocated_bytes)
    )?;
    outln!("Command: {}", compact::command(&c.path, algo, undo))?;
    if a.has("dry-run") || !cfg!(windows) {
        if !cfg!(windows) {
            outln!("agentdisk runs compression only on Windows; run the command above yourself.")?;
        }
        return Ok(ExitCode::SUCCESS);
    }
    if !human_confirms(if undo { "decompression" } else { "compression" }, "yes")? {
        outln!("Cancelled. Nothing changed.")?;
        return Ok(ExitCode::SUCCESS);
    }
    let after = compact::run(&c, algo, undo)?;
    out!("{}", compact::report(&c.before, &after))?;
    Ok(ExitCode::SUCCESS)
}

fn cmd_check(rest: &[String]) -> Result<ExitCode, CliError> {
    let a = parse(rest, &["min-free-gb", "max-worktrees"], &["json", "quiet"])?;
    if a.has("help") {
        outln!("agentdisk check [--min-free-gb 20] [--max-worktrees 20] [--json] [--quiet]\n\nFast. Prints a warning when space is low or agent worktrees pile up.\nLimits default to min_free_gb and max_worktrees in config.toml (both 20 when unset).\nAlways exits 0, so it is safe in hooks. --quiet prints only warnings.")?;
        return Ok(ExitCode::SUCCESS);
    }
    no_positionals(&a, None)?;
    let cfg = config::load();
    print_warnings(&cfg);
    let h = check::run(
        a.num("min-free-gb")?.unwrap_or(cfg.min_free_gb),
        usize::try_from(a.num("max-worktrees")?.unwrap_or(cfg.max_worktrees as u64))
            .unwrap_or(usize::MAX),
        &cfg.adapters,
    );
    if a.has("json") {
        outln!("{}", check::to_json(&h))?;
    } else if !(a.has("quiet") && h.warnings.is_empty()) {
        out!("{}", check::render(&h))?;
    }
    Ok(ExitCode::SUCCESS)
}

fn cmd_adapters(rest: &[String]) -> Result<ExitCode, CliError> {
    let a = parse(rest, &["adapters"], &["json"])?;
    if a.has("help") {
        outln!("agentdisk adapters [--adapters <dir>]... [--json]\n\nList known agents and their data locations.")?;
        return Ok(ExitCode::SUCCESS);
    }
    no_positionals(&a, None)?;
    let cfg = config::load();
    let mut dirs = cfg.adapters.clone();
    dirs.extend(a.many("adapters").into_iter().map(PathBuf::from));
    let (list, warnings) = scan::load_adapters(&dirs);
    for w in warnings {
        eprintln!("warning: {w}");
    }
    if a.has("json") {
        use agentdisk_core::json::Value;
        let v: Vec<Value> = list
            .iter()
            .map(|ad| {
                Value::obj(vec![
                    ("id", ad.id.as_str().into()),
                    ("name", ad.name.as_str().into()),
                    ("vendor", ad.vendor.as_str().into()),
                    ("verified", ad.verified.clone().into()),
                    (
                        "paths",
                        ad.paths
                            .iter()
                            .map(|p| {
                                Value::obj(vec![
                                    ("id", p.id.as_str().into()),
                                    ("template", p.template.as_str().into()),
                                    ("class", p.class.as_str().into()),
                                    ("documented", p.documented.into()),
                                    (
                                        "found",
                                        (!agentdisk_core::paths::expand(&p.template).is_empty())
                                            .into(),
                                    ),
                                ])
                            })
                            .collect::<Vec<_>>()
                            .into(),
                    ),
                ])
            })
            .collect();
        outln!("{}", Value::Arr(v).pretty())?;
        return Ok(ExitCode::SUCCESS);
    }
    outln!(
        "Agents agentdisk knows ({}). ✓ = found on this computer.\n",
        list.len()
    )?;
    for ad in &list {
        let found: Vec<&adapters::PathEntry> = ad
            .paths
            .iter()
            .filter(|p| p.applies_here() && !agentdisk_core::paths::expand(&p.template).is_empty())
            .collect();
        outln!(
            "{} {} — {} [{}]",
            if found.is_empty() { " " } else { "✓" },
            ad.name,
            ad.vendor,
            ad.verified.as_deref().unwrap_or("unverified")
        )?;
        for p in ad.paths.iter().filter(|p| p.applies_here()) {
            let here = found.iter().any(|f| f.id == p.id);
            outln!(
                "    {} {:<20} {:<12} {}{}",
                if here { "✓" } else { "·" },
                p.id,
                p.class.as_str(),
                p.template,
                if p.documented { "" } else { "  (undocumented)" }
            )?;
        }
    }
    outln!(
        "\nAdd an agent: put a .toml file in {}",
        display(&agentdisk_core::paths::config_dir().join("adapters"))
    )?;
    Ok(ExitCode::SUCCESS)
}

fn cmd_config(rest: &[String]) -> Result<ExitCode, CliError> {
    let a = parse(rest, &[], &["init"])?;
    if a.has("help") {
        outln!("agentdisk config [--init]\n\nShow configuration paths and settings. --init writes an example file.")?;
        return Ok(ExitCode::SUCCESS);
    }
    no_positionals(&a, None)?;
    let path = config::path();
    if a.has("init") {
        if path.exists() {
            return Err(format!("{} already exists", display(&path)).into());
        }
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, config::EXAMPLE).map_err(|e| e.to_string())?;
        outln!("Wrote {}", display(&path))?;
        return Ok(ExitCode::SUCCESS);
    }
    outln!(
        "Config file: {}{}",
        display(&path),
        if path.exists() {
            ""
        } else {
            " (not created yet; use --init)"
        }
    )?;
    outln!(
        "Data folder: {}",
        display(&agentdisk_core::paths::data_dir())
    )?;
    let cfg = config::load();
    print_warnings(&cfg);
    outln!(
        "projects: {:?}",
        cfg.projects.iter().map(|p| display(p)).collect::<Vec<_>>()
    )?;
    outln!(
        "worktree_days = {}, build_days = {}, max_depth = {}",
        cfg.worktree_days,
        cfg.build_days,
        cfg.max_depth
    )?;
    outln!(
        "min_free_gb = {}, max_worktrees = {}",
        cfg.min_free_gb,
        cfg.max_worktrees
    )?;
    Ok(ExitCode::SUCCESS)
}

fn run(args: &[String]) -> Result<ExitCode, CliError> {
    let Some(cmd) = args.first() else {
        outln!("{HELP}")?;
        return Ok(ExitCode::SUCCESS);
    };
    let rest = &args[1..];
    match cmd.as_str() {
        "-h" | "--help" | "help" => {
            outln!("{HELP}")?;
            Ok(ExitCode::SUCCESS)
        }
        "-V" | "--version" | "version" => {
            outln!("agentdisk {VERSION}")?;
            Ok(ExitCode::SUCCESS)
        }
        "scan" => cmd_scan(rest),
        "size" => cmd_size(rest),
        "explain" => cmd_explain(rest),
        "advise" => cmd_advise(rest),
        "plan" => cmd_plan(rest),
        "plans" => cmd_plans(rest),
        "apply" => cmd_apply(rest),
        "restore" => cmd_restore(rest),
        "purge" => cmd_purge(rest),
        "quarantine" => {
            out!("{}", cleanup::render_quarantine())?;
            Ok(ExitCode::SUCCESS)
        }
        "compact" => cmd_compact(rest),
        "check" => cmd_check(rest),
        "adapters" => cmd_adapters(rest),
        "config" => cmd_config(rest),
        "mcp" => {
            let a = parse(rest, &[], &[])?;
            if a.has("help") {
                outln!("agentdisk mcp\n\nRun the MCP server on standard input and output.")?;
                return Ok(ExitCode::SUCCESS);
            }
            no_positionals(&a, None)?;
            mcp::serve()?;
            Ok(ExitCode::SUCCESS)
        }
        other => Err(format!("unknown command '{other}'. Run `agentdisk --help`.").into()),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(code) => code,
        Err(CliError::Output(e)) if e.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(2)
        }
    }
}
